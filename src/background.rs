//! Native notification-area controls and activation-only, per-profile IPC.
//!
//! This module owns no browser data or download credentials. Closing the
//! visible browser is handled by the main loop; its native downloads stay on
//! that same UI thread. A later launch only signals the existing instance.
use crate::Msg;
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    mem::size_of,
    os::windows::ffi::{OsStrExt, OsStringExt},
    os::windows::fs::OpenOptionsExt,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicIsize, AtomicU32, Ordering},
    thread::{self, JoinHandle},
};
use tao::{event_loop::EventLoopProxy, platform::windows::WindowExtWindows, window::Window};
use windows::{
    core::{w, BOOL, HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, HINSTANCE, HWND,
            LPARAM, POINT, WAIT_OBJECT_0, WPARAM,
        },
        Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE},
        Storage::FileSystem::{
            GetFileInformationByHandle, MoveFileExW, BY_HANDLE_FILE_INFORMATION,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{
                CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenEventW,
                OpenProcess, QueryFullProcessImageNameW, ResetEvent, SetEvent,
                WaitForMultipleObjects, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                SYNCHRONIZATION_SYNCHRONIZE,
            },
        },
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP,
                NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS,
                NIM_SETVERSION, NINF_KEY, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICONDATAW,
                NOTIFYICON_VERSION_4,
            },
            WindowsAndMessaging::{
                AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu,
                EnumWindows, GetClassNameW, GetCursorPos, GetWindow, GetWindowThreadProcessId,
                IsIconic, IsWindowVisible, LoadImageW, PostMessageW, RegisterWindowMessageW,
                SetForegroundWindow, ShowWindowAsync, TrackPopupMenu, GW_OWNER, HICON, HMENU,
                IMAGE_ICON, LR_DEFAULTSIZE, MF_SEPARATOR, MF_STRING, MSG, SW_RESTORE, TPM_NONOTIFY,
                TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_ENDSESSION, WM_NULL,
                WM_RBUTTONUP,
            },
        },
    },
};

const CALLBACK: u32 = WM_APP + 0x439;
const ICON_ID: u32 = 1;
const OPEN: u32 = 1;
const DOWNLOADS: u32 = 2;
const EXIT: u32 = 3;
static TRAY_OWNER: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct KernelEvent(HANDLE);
// Kernel event handles may be waited on and closed on any thread. Only the
// activation handle is transferred; Instance retains the stop handle until
// its worker has joined.
unsafe impl Send for KernelEvent {}
impl Drop for KernelEvent {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn file_identity(file: &File) -> Option<(u32, u64)> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information) }.ok()?;
    Some((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

fn activation_name(root: &Path) -> Result<String, String> {
    let canonical =
        fs::canonicalize(root).map_err(|e| format!("Could not identify Yas profile: {e}"))?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
        .open(&canonical)
        .map_err(|e| format!("Could not identify Yas profile directory: {e}"))?;
    let mut identity = b"Yas.profile.activation.v1\0".to_vec();
    if let Some((volume, index)) = file_identity(&directory) {
        // Same physical directory, including case/short-name aliases, maps to
        // one event. Different profiles cannot steal each other's activation.
        identity.extend_from_slice(b"file-id\0");
        identity.extend_from_slice(&volume.to_le_bytes());
        identity.extend_from_slice(&index.to_le_bytes());
    } else {
        identity.extend_from_slice(b"canonical-path\0");
        for unit in canonical.as_os_str().encode_wide() {
            identity.extend_from_slice(&unit.to_le_bytes());
        }
    }
    let mut digest = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, &identity, &mut digest) };
    if status.0 < 0 {
        return Err(format!(
            "Could not identify Yas activation channel: 0x{:08x}",
            status.0 as u32
        ));
    }
    let encoded: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("Local\\Yas.Activate.{encoded}"))
}

const OWNER_FILE: &str = ".yas-activation.pid";

fn process_started(process: HANDLE) -> Result<u64, String> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|e| e.to_string())?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn read_owner(path: &Path) -> Option<String> {
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(1024)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

struct ActivationOwner {
    path: PathBuf,
    text: String,
}
impl ActivationOwner {
    fn create(root: &Path, name: &str) -> Result<Self, String> {
        let pid = unsafe { GetCurrentProcessId() };
        let started = process_started(unsafe { GetCurrentProcess() })?;
        let text = format!("Yas activation v1\n{pid}\n{started}\n{name}\n");
        let path = root.join(OWNER_FILE);
        let temporary = root.join(format!("{OWNER_FILE}.{pid}.{started}.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| e.to_string())?;
            file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            unsafe {
                MoveFileExW(
                    &HSTRING::from(temporary.as_os_str()),
                    &HSTRING::from(path.as_os_str()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
            .map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(format!("Could not publish Yas activation owner: {error}"));
        }
        Ok(Self { path, text })
    }
}
impl Drop for ActivationOwner {
    fn drop(&mut self) {
        if read_owner(&self.path).as_deref() == Some(self.text.as_str()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct ExistingOwner {
    process: KernelEvent,
    pid: u32,
}

/// The record is never used for commands or HWNDs. Only a live process with
/// the recorded creation time and this same executable may receive the
/// launcher's foreground permission. Stale/replaced records simply skip it.
fn existing_owner(root: &Path, name: &str) -> Option<ExistingOwner> {
    let text = read_owner(&root.join(OWNER_FILE))?;
    let mut lines = text.lines();
    if lines.next() != Some("Yas activation v1") {
        return None;
    }
    let pid = lines.next()?.parse::<u32>().ok()?;
    let started = lines.next()?.parse::<u64>().ok()?;
    if lines.next() != Some(name) || lines.next().is_some() {
        return None;
    }
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .ok()?;
    let process = KernelEvent(process);
    if process_started(process.0).ok() != Some(started)
        || unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0
    {
        return None;
    }
    let mut executable = vec![0u16; 32768];
    let mut length = executable.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(executable.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    let executable = PathBuf::from(OsString::from_wide(&executable[..length as usize]));
    let own_executable = std::env::current_exe().ok()?;
    let existing = File::open(executable).ok()?;
    let own = File::open(own_executable).ok()?;
    if file_identity(&existing).is_none() || file_identity(&existing) != file_identity(&own) {
        return None;
    }
    Some(ExistingOwner { process, pid })
}

fn wait_for_acknowledgement(owner: HANDLE, acknowledgement: HANDLE, timeout: u32) -> bool {
    // A terminated owner wins if both handles are signalled. In particular,
    // an old ACK must never turn an exit race into a successful activation.
    let result = unsafe { WaitForMultipleObjects(&[owner, acknowledgement], false, timeout) };
    result.0 == WAIT_OBJECT_0.0 + 1 && unsafe { WaitForSingleObject(owner, 0) } != WAIT_OBJECT_0
}

struct VisibleWindow {
    pid: u32,
    window: HWND,
}
unsafe extern "system" fn find_visible_window(hwnd: HWND, parameter: LPARAM) -> BOOL {
    let result = &mut *(parameter.0 as *mut VisibleWindow);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != result.pid
        || !IsWindowVisible(hwnd).as_bool()
        || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid())
    {
        return BOOL(1);
    }
    let mut class = [0u16; 128];
    let length = GetClassNameW(hwnd, &mut class);
    // Tao 0.37's pinned default top-level class. Native Save dialogs and
    // WebView child windows must never masquerade as a reopened browser.
    if String::from_utf16_lossy(&class[..length.max(0) as usize]) == "Window Class" {
        result.window = hwnd;
        return BOOL(0);
    }
    BOOL(1)
}

fn foreground_visible_owner(owner: &ExistingOwner) -> bool {
    if unsafe { WaitForSingleObject(owner.process.0, 0) } == WAIT_OBJECT_0 {
        return false;
    }
    let mut result = VisibleWindow {
        pid: owner.pid,
        window: HWND::default(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(find_visible_window),
            LPARAM(&mut result as *mut _ as isize),
        );
    }
    if result.window.is_invalid() {
        return false;
    }
    unsafe {
        if IsIconic(result.window).as_bool() {
            let _ = ShowWindowAsync(result.window, SW_RESTORE);
        }
        let _ = SetForegroundWindow(result.window);
    }
    (unsafe { WaitForSingleObject(owner.process.0, 0) }) != WAIT_OBJECT_0
}

/// Signal an already-running instance of this exact profile. This is only an
/// activation request; it never imports data or carries a URL or command.
pub fn request_existing(root: &Path) -> bool {
    let Ok(name) = activation_name(root) else {
        return false;
    };
    let Ok(event) =
        (unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name.as_str())) })
    else {
        return false;
    };
    let event = KernelEvent(event);
    let Some(owner) = existing_owner(root, &name) else {
        return false;
    };
    unsafe {
        let _ = AllowSetForegroundWindow(owner.pid);
    }
    let Ok(acknowledgement) = (unsafe {
        OpenEventW(
            EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
            false,
            &HSTRING::from(format!("{name}.ack")),
        )
    }) else {
        return foreground_visible_owner(&owner);
    };
    let acknowledgement = KernelEvent(acknowledgement);
    if unsafe { ResetEvent(acknowledgement.0) }.is_err() || unsafe { SetEvent(event.0) }.is_err() {
        return false;
    }
    if wait_for_acknowledgement(owner.process.0, acknowledgement.0, 2000) {
        return true;
    }
    // A visible browser can be inside a native modal Save dialog, which
    // postpones Tao's user event. Only that already-visible main window is
    // accepted as a bounded fallback; an exiting hidden owner is never one.
    foreground_visible_owner(&owner)
}

pub struct Instance {
    stop: KernelEvent,
    acknowledgement: KernelEvent,
    worker: Option<JoinHandle<()>>,
    _owner: ActivationOwner,
}
impl Instance {
    /// Start listening while holding the browser's exclusive profile lock.
    /// That lock permits safe reuse of names retained by exiting launchers.
    pub fn listen(root: &Path, proxy: EventLoopProxy<Msg>) -> Result<Self, String> {
        Self::listen_with(root, move || proxy.send_event(Msg::Reopen).is_ok())
    }

    /// Call only after the UI has cleared background mode and shown its main
    /// window. Queuing a Reopen event alone does not acknowledge activation.
    pub fn acknowledge(&self) {
        unsafe {
            let _ = SetEvent(self.acknowledgement.0);
        }
    }

    fn listen_with<F>(root: &Path, mut activate: F) -> Result<Self, String>
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let name = activation_name(root)?;
        let activation = unsafe { CreateEventW(None, false, false, &HSTRING::from(name.as_str())) }
            .map_err(|e| format!("Could not create Yas activation channel: {e}"))?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let activation = KernelEvent(activation);
        if already_exists && existing_owner(root, &name).is_some() {
            return Err("Another Yas activation listener already owns this profile.".into());
        }
        // A launcher may briefly retain the old event handle while its owner
        // exits. Once no validated owner remains, the profile-lock holder can
        // reuse the channel rather than fail on that harmless lingering name.
        unsafe { ResetEvent(activation.0) }.map_err(|e| e.to_string())?;
        let acknowledgement = KernelEvent(
            unsafe { CreateEventW(None, true, false, &HSTRING::from(format!("{name}.ack"))) }
                .map_err(|e| format!("Could not create Yas activation acknowledgement: {e}"))?,
        );
        unsafe { ResetEvent(acknowledgement.0) }.map_err(|e| e.to_string())?;
        let stop = KernelEvent(
            unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
                .map_err(|e| format!("Could not create Yas activation shutdown event: {e}"))?,
        );
        let owner = ActivationOwner::create(root, &name)?;
        // The stop event remains owned by Instance until Drop has joined the
        // thread. Passing its numeric value does not transfer ownership.
        let stop_value = stop.0 .0 as usize;
        let worker = thread::Builder::new()
            .name("yas-activation".into())
            .spawn(move || {
                let handles = [HANDLE(stop_value as *mut _), activation.0];
                loop {
                    let result = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                    if result == WAIT_OBJECT_0 {
                        break;
                    }
                    if result.0 != WAIT_OBJECT_0.0 + 1 || !activate() {
                        break;
                    }
                }
                drop(activation);
            })
            .map_err(|e| format!("Could not start Yas activation listener: {e}"))?;
        Ok(Self {
            stop,
            acknowledgement,
            worker: Some(worker),
            _owner: owner,
        })
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn copy_text<const N: usize>(destination: &mut [u16; N], text: &str) {
    destination.fill(0);
    let mut offset = 0;
    for character in text.chars() {
        let mut encoded = [0u16; 2];
        let units = character.encode_utf16(&mut encoded);
        if offset + units.len() >= N {
            break;
        }
        destination[offset..offset + units.len()].copy_from_slice(units);
        offset += units.len();
    }
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    visible: Cell<bool>,
    tip: RefCell<String>,
}
impl Tray {
    pub fn new(window: &Window) -> Result<Self, String> {
        let hwnd = HWND(window.hwnd() as *mut _);
        let created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if created == 0 {
            return Err(format!(
                "Could not register Yas tray recovery: {}",
                std::io::Error::last_os_error()
            ));
        }
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }.map_err(|e| e.to_string())?;
        // No LR_SHARED: this HICON is owned and is destroyed with Tray.
        let icon = unsafe {
            // Win32 MAKEINTRESOURCEW(1): this is a resource identifier, never
            // a dereferenced Rust pointer. dangling::<u16>() would select ID 2.
            #[expect(
                clippy::manual_dangling_ptr,
                reason = "Win32 integer resource identifier 1, not a dereferenceable pointer."
            )]
            let icon_resource = PCWSTR(1usize as *const u16);
            LoadImageW(
                Some(HINSTANCE(module.0)),
                icon_resource,
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE,
            )
        }
        .map_err(|e| format!("Could not load the Yas tray icon: {e}"))?;
        if TRAY_OWNER
            .compare_exchange(0, window.hwnd(), Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            unsafe {
                let _ = DestroyIcon(HICON(icon.0));
            }
            return Err("This Yas process already owns a tray icon.".into());
        }
        TASKBAR_CREATED.store(created, Ordering::SeqCst);
        Ok(Self {
            hwnd,
            icon: HICON(icon.0),
            visible: Cell::new(false),
            tip: RefCell::new("Yas".into()),
        })
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            ..Default::default()
        }
    }

    fn add(&self) -> Result<(), String> {
        let mut data = self.data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = self.icon;
        copy_text(&mut data.szTip, &self.tip.borrow());
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
            return Err("Windows could not show the Yas background-download icon.".into());
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        if !unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) }.as_bool() {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
            return Err("Windows could not enable Yas tray controls.".into());
        }
        self.visible.set(true);
        Ok(())
    }

    /// Caller must not hide the browser unless this succeeds.
    pub fn show(&self, tip: &str) -> Result<(), String> {
        *self.tip.borrow_mut() = tip.to_owned();
        if self.visible.get() {
            let mut data = self.data();
            data.uFlags = NIF_TIP | NIF_SHOWTIP;
            copy_text(&mut data.szTip, tip);
            if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
                return Ok(());
            }
        }
        self.add()
    }

    pub fn hide(&self) {
        if self.visible.replace(false) {
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            }
        }
    }

    pub fn restore(&self) -> Result<(), String> {
        if self.visible.get() {
            self.add()
        } else {
            Ok(())
        }
    }

    pub fn balloon(&self, title: &str, body: &str) -> Result<(), String> {
        if !self.visible.get() {
            return Ok(());
        }
        let mut data = self.data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO | NIIF_RESPECT_QUIET_TIME;
        copy_text(&mut data.szInfoTitle, title);
        copy_text(&mut data.szInfo, body);
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) }.as_bool() {
            Ok(())
        } else {
            Err("Windows could not display the Yas download notification.".into())
        }
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        self.hide();
        let _ = TRAY_OWNER.compare_exchange(
            self.hwnd.0 as isize,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        unsafe {
            let _ = DestroyIcon(self.icon);
        }
    }
}

struct Popup(HMENU);
impl Drop for Popup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyMenu(self.0);
        }
    }
}

unsafe fn menu(hwnd: HWND, anchor: WPARAM) -> Option<u32> {
    let popup = Popup(CreatePopupMenu().ok()?);
    AppendMenuW(popup.0, MF_STRING, OPEN as usize, w!("Open Yas")).ok()?;
    AppendMenuW(popup.0, MF_STRING, DOWNLOADS as usize, w!("Downloads")).ok()?;
    AppendMenuW(popup.0, MF_SEPARATOR, 0, PCWSTR::null()).ok()?;
    AppendMenuW(
        popup.0,
        MF_STRING,
        EXIT as usize,
        w!("Exit and stop downloads"),
    )
    .ok()?;
    // Version 4 supplies the icon's screen anchor for both mouse and keyboard
    // invocation. Preserve signed coordinates on monitors left of the primary.
    let mut point = POINT {
        x: (anchor.0 as u16 as i16) as i32,
        y: ((anchor.0 >> 16) as u16 as i16) as i32,
    };
    if point.x == -1 && point.y == -1 {
        GetCursorPos(&mut point).ok()?;
    }
    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        popup.0,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        None,
        hwnd,
        None,
    )
    .0 as u32;
    // Required by the notification-area context-menu focus contract.
    let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
    if command == 0 {
        let data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: ICON_ID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_SETFOCUS, &data);
    }
    Some(command)
}

/// Install before constructing the event loop with `with_msg_hook`.
/// The builder has no proxy yet; the caller fills the slot immediately after
/// building the event loop and before constructing the browser window.
pub fn hook(
    proxy: Rc<RefCell<Option<EventLoopProxy<Msg>>>>,
) -> impl FnMut(*const std::ffi::c_void) -> bool {
    move |raw| {
        if raw.is_null() {
            return false;
        }
        let message = unsafe { &*(raw as *const MSG) };
        let owner = TRAY_OWNER.load(Ordering::SeqCst);
        if owner == 0 || message.hwnd.0 as isize != owner {
            return false;
        }
        let send = |message| {
            if let Some(proxy) = proxy.borrow().as_ref() {
                let _ = proxy.send_event(message);
            }
        };
        // Defensive handling for queued end-session messages. Real Windows
        // shutdown sends this synchronously to Tao's WndProc, which emits
        // LoopDestroyed and exits directly; the caller must checkpoint there.
        if message.message == WM_ENDSESSION && message.wParam.0 != 0 {
            send(Msg::Exit);
            return true;
        }
        let restored = TASKBAR_CREATED.load(Ordering::SeqCst);
        if restored != 0 && message.message == restored {
            send(Msg::RestoreTray);
            return true;
        }
        if message.message != CALLBACK {
            return false;
        }
        let packed = message.lParam.0 as usize;
        if ((packed >> 16) & 0xffff) as u32 != ICON_ID {
            return true;
        }
        match (packed & 0xffff) as u32 {
            NIN_SELECT => send(Msg::Reopen),
            value if value == (NIN_SELECT | NINF_KEY) => send(Msg::Reopen),
            NIN_BALLOONUSERCLICK => send(Msg::ShowDownloads),
            WM_CONTEXTMENU | WM_RBUTTONUP => match unsafe { menu(message.hwnd, message.wParam) } {
                Some(OPEN) => send(Msg::Reopen),
                Some(DOWNLOADS) => send(Msg::ShowDownloads),
                Some(EXIT) => send(Msg::Exit),
                _ => {}
            },
            _ => {}
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn root() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "yas-background-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn activation_is_isolated_by_profile_and_listener_drop_releases_channel() {
        let a = root();
        let b = root();
        assert_eq!(
            activation_name(&a).unwrap(),
            activation_name(&a.join(".")).unwrap()
        );
        assert_ne!(activation_name(&a).unwrap(), activation_name(&b).unwrap());
        let (send, receive) = mpsc::channel();
        let instance = Instance::listen_with(&a, move || send.send(()).is_ok()).unwrap();
        assert!(a.join(OWNER_FILE).is_file());
        assert!(!request_existing(&b));
        for _ in 0..2 {
            let requested = a.clone();
            let launcher = thread::spawn(move || request_existing(&requested));
            receive.recv_timeout(Duration::from_secs(3)).unwrap();
            instance.acknowledge();
            assert!(launcher.join().unwrap());
        }
        assert!(Instance::listen_with(&a, || true).is_err());
        drop(instance);
        assert!(!request_existing(&a));
        assert!(!a.join(OWNER_FILE).exists());
        fs::remove_dir_all(a).unwrap();
        fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn listener_shutdown_does_not_require_an_activation_or_polling() {
        let path = root();
        let instance =
            Instance::listen_with(&path, || panic!("No activation was requested")).unwrap();
        drop(instance);
        assert!(!request_existing(&path));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn stale_owner_record_cannot_prevent_a_new_listener() {
        let path = root();
        fs::write(path.join(OWNER_FILE), "obsolete process record").unwrap();
        let instance = Instance::listen_with(&path, || true).unwrap();
        let published = read_owner(&path.join(OWNER_FILE)).unwrap();
        assert!(published.starts_with("Yas activation v1\n"));
        assert!(published.contains(&activation_name(&path).unwrap()));
        drop(instance);
        assert!(!path.join(OWNER_FILE).exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn owner_exit_wins_over_acknowledgement_and_unacknowledged_requests_fail() {
        // Manual-reset kernel events stand in for the signalled process
        // handle and ACK, so no child process or native window is needed.
        let exited =
            KernelEvent(unsafe { CreateEventW(None, true, true, PCWSTR::null()) }.unwrap());
        let acknowledgement =
            KernelEvent(unsafe { CreateEventW(None, true, true, PCWSTR::null()) }.unwrap());
        assert!(!wait_for_acknowledgement(exited.0, acknowledgement.0, 0));
        unsafe { ResetEvent(exited.0) }.unwrap();
        assert!(wait_for_acknowledgement(exited.0, acknowledgement.0, 0));
        unsafe { ResetEvent(acknowledgement.0) }.unwrap();
        assert!(!wait_for_acknowledgement(exited.0, acknowledgement.0, 0));
    }

    #[test]
    fn old_launcher_handles_do_not_block_a_replacement_listener() {
        let path = root();
        let first = Instance::listen_with(&path, || true).unwrap();
        let name = activation_name(&path).unwrap();
        let retained = KernelEvent(
            unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &HSTRING::from(name.as_str())) }
                .unwrap(),
        );
        drop(first);
        assert!(!request_existing(&path));
        let replacement = Instance::listen_with(&path, || true).unwrap();
        drop(replacement);
        drop(retained);
        assert!(!request_existing(&path));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn tray_text_truncation_preserves_surrogate_pairs_and_terminator() {
        let mut text = [0u16; 4];
        copy_text(&mut text, "ab🦅more");
        assert_eq!(String::from_utf16(&text[..2]).unwrap(), "ab");
        assert_eq!(&text[2..], &[0, 0]);
        copy_text(&mut text, "🦅x");
        assert_eq!(String::from_utf16(&text[..3]).unwrap(), "🦅x");
        assert_eq!(text[3], 0);
    }
}
