//! Yas-owned, CPU-only Ollama child and explicit first-use model setup.
//! No service, global environment change, external installer or cloud inference.
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    net::TcpListener,
    os::windows::{fs::MetadataExt, io::AsRawHandle, process::CommandExt},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::Cryptography::{
        BCryptCreateHash, BCryptDestroyHash, BCryptFinishHash, BCryptHashData, BCRYPT_HASH_HANDLE,
        BCRYPT_SHA256_ALG_HANDLE,
    },
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

pub const OWNED_ENDPOINT: &str = "http://127.0.0.1:11435";
pub const SETUP_MODEL: &str = "qwen3:0.6b";
pub const MODEL_BYTES: u64 = 522_653_767;
pub(super) const MODEL_MANIFEST: &str =
    "7df6b6e09427a769808717c0a93cadc4ae99ed4eb8bf5ca557c90846becea435";
const MANIFEST: &[u8] = include_bytes!("../ai-runtime/AI_RUNTIME_MANIFEST.json");
const MAX_PULL_BYTES: u64 = 16 * 1024 * 1024;
const REPARSE_POINT: u32 = 0x400;
const MODEL_PARTS: [(&str, u64); 5] = [
    (
        "7f4030143c1c477224c5434f8272c662a8b042079a0a584f0a27a1684fe2e1fa",
        522_640_096,
    ),
    (
        "ae370d884f108d16e7cc8fd5259ebc5773a0afa6e078b11f4ed7e39a27e0dfc4",
        1723,
    ),
    (
        "d18a5cc71b84bc4af394a31116bd3932b42241de70c77d2b76d69a314ec8aa12",
        11338,
    ),
    (
        "cff3f395ef3756ab63e58b0ad1b32bb6f802905cae1472e6a12034e4246fbbdb",
        120,
    ),
    (
        "b0830f4ff6a0220cfd995455206353b0ed23c0aee865218b154b7a75087b4e55",
        490,
    ),
];

#[derive(Clone, Debug, Serialize)]
pub struct SetupProgress {
    pub status: String,
    pub completed: u64,
    pub total: u64,
}

struct Job(usize);
impl Job {
    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }
    fn new() -> Result<Self, String> {
        let handle = unsafe { CreateJobObjectW(None, None) }
            .map_err(|_| "Could not create a private AI process group.".to_string())?;
        let job = Self(handle.0 as usize);
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(|_| "Could not isolate Yas’s local AI process.".to_string())?;
        Ok(job)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}
struct Owned {
    child: Child,
    job: Option<Job>,
    verified_model: bool,
}
impl Drop for Owned {
    fn drop(&mut self) {
        self.job.take(); // KILL_ON_JOB_CLOSE owns this exact server and its runners.
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
static OWNED: OnceLock<Mutex<Option<Owned>>> = OnceLock::new();
static STORAGE: OnceLock<PathBuf> = OnceLock::new();

/// The browser supplies its explicitly locked profile, including isolated QA
/// profiles. No fallback silently writes to another user's/default profile.
pub fn configure_storage(profile: &Path) -> Result<(), String> {
    let profile = fs::canonicalize(profile)
        .map_err(|_| "Could not locate the local AI profile.".to_string())?;
    let home = profile.join("ai");
    if let Some(existing) = STORAGE.get() {
        return if *existing == home {
            Ok(())
        } else {
            Err("Local AI already belongs to another browser profile.".into())
        };
    }
    STORAGE
        .set(home)
        .map_err(|_| "Local AI profile is already configured.".into())
}

fn runtime_home() -> Result<PathBuf, String> {
    let root = STORAGE
        .get()
        .ok_or("Yas's local AI profile is not ready.")?
        .clone();
    fs::create_dir_all(root.join("models"))
        .map_err(|_| "Could not create local AI storage.".to_string())?;
    fs::create_dir_all(root.join("tmp"))
        .map_err(|_| "Could not create local AI temporary storage.".to_string())?;
    Ok(root)
}
fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['\\', ':'])
        && Path::new(name)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn plain_file(path: &Path, expected: u64) -> Result<(), String> {
    let info = fs::symlink_metadata(path)
        .map_err(|_| "A local AI file is missing. Reinstall Yas or run AI setup.".to_string())?;
    if !info.is_file() || info.file_attributes() & REPARSE_POINT != 0 || info.len() != expected {
        return Err("A local AI file is invalid. Reinstall Yas or run AI setup.".into());
    }
    Ok(())
}
struct Hash(BCRYPT_HASH_HANDLE);
impl Drop for Hash {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.0);
        }
    }
}
fn file_sha(path: &Path, cancel: Option<&AtomicBool>) -> Result<String, String> {
    let mut raw = BCRYPT_HASH_HANDLE::default();
    if unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut raw, None, None, 0) }.0 < 0 {
        return Err("Could not verify a local AI file.".into());
    }
    let hash = Hash(raw);
    let mut file = File::open(path).map_err(|_| "Could not read a local AI file.".to_string())?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("Local AI setup cancelled.".into());
        }
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Could not verify a local AI file.".to_string())?;
        if count == 0 {
            break;
        }
        if unsafe { BCryptHashData(hash.0, &buffer[..count], 0) }.0 < 0 {
            return Err("Could not verify a local AI file.".into());
        }
    }
    let mut digest = [0; 32];
    if unsafe { BCryptFinishHash(hash.0, &mut digest, 0) }.0 < 0 {
        return Err("Could not finish local AI verification.".into());
    }
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn verify_runtime() -> Result<PathBuf, String> {
    let folder = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join("ai-runtime")))
        .ok_or("Could not locate Yas’s local AI runtime.")?;
    verify_runtime_at(&folder)?;
    Ok(folder)
}
fn verify_runtime_at(folder: &Path) -> Result<(), String> {
    let manifest: Value = serde_json::from_slice(MANIFEST)
        .map_err(|_| "This Yas build has an invalid AI manifest.".to_string())?;
    if manifest["schema"] != 1 || manifest["cpuOnly"] != true {
        return Err("This Yas AI runtime is unsupported.".into());
    }
    let files = manifest["files"]
        .as_array()
        .ok_or("This Yas build has no AI payload.")?;
    let mut names = HashSet::new();
    for file in files {
        let name = file["path"]
            .as_str()
            .ok_or("Invalid AI runtime file name.")?;
        if !safe_relative(name) || !names.insert(name.to_ascii_lowercase()) {
            return Err("Invalid AI runtime payload path.".into());
        }
        let expected = file["bytes"]
            .as_u64()
            .ok_or("Invalid AI runtime file length.")?;
        let digest = file["sha256"]
            .as_str()
            .ok_or("Invalid AI runtime file checksum.")?;
        let path = folder.join(name);
        plain_file(&path, expected)?;
        if !file_sha(&path, None)?.eq_ignore_ascii_case(digest) {
            return Err("Yas’s bundled AI runtime failed verification. Reinstall Yas.".into());
        }
    }
    fn check_directory(path: &Path, root: &Path, names: &HashSet<String>) -> Result<(), String> {
        for item in fs::read_dir(path)
            .map_err(|_| "Could not check the AI runtime directory.".to_string())?
        {
            let item = item.map_err(|_| "Could not check an AI runtime file.".to_string())?;
            let meta = item
                .path()
                .symlink_metadata()
                .map_err(|_| "Could not check an AI runtime file.".to_string())?;
            if meta.file_attributes() & REPARSE_POINT != 0 {
                return Err("Linked AI runtime paths are not supported.".into());
            }
            if meta.is_dir() {
                check_directory(&item.path(), root, names)?;
            } else if item
                .path()
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll") || e.eq_ignore_ascii_case("exe"))
            {
                let relative = item
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| "Invalid AI runtime location.")?
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_ascii_lowercase();
                if !names.contains(&relative) {
                    return Err(
                        "An unexpected executable was found in Yas’s AI runtime. Reinstall Yas."
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
    check_directory(folder, folder, &names)?;
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TcpRow {
    _state: u32,
    local_address: u32,
    local_port: u32,
    remote_address: u32,
    remote_port: u32,
    pid: u32,
}
#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetExtendedTcpTable(
        table: *mut core::ffi::c_void,
        size: *mut u32,
        order: i32,
        family: u32,
        class: u32,
        reserved: u32,
    ) -> u32;
}
fn owns_listener(pid: u32) -> bool {
    owns_listener_on(pid, 11435)
}
fn owns_listener_on(pid: u32, port: u16) -> bool {
    // This table class already restricts results to listening endpoints.
    // Some Windows builds return state 0/6 for a verified live listener, so
    // do not reinterpret its redundant state field. Exact owner/address/port
    // checks still precede every request to the Yas-owned server.
    const OWNER_PID_LISTENER: u32 = 3;
    let mut size = 0;
    let status = unsafe {
        GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, OWNER_PID_LISTENER, 0)
    };
    if status != 122 || !(4..=16 * 1024 * 1024).contains(&size) {
        return false;
    }
    let mut bytes = vec![0u8; size as usize];
    if unsafe {
        GetExtendedTcpTable(
            bytes.as_mut_ptr().cast(),
            &mut size,
            0,
            2,
            OWNER_PID_LISTENER,
            0,
        )
    } != 0
        || size as usize > bytes.len()
    {
        return false;
    }
    listener_table_owns(&bytes[..size as usize], pid, port)
}
fn listener_table_owns(bytes: &[u8], pid: u32, port: u16) -> bool {
    if bytes.len() < 4 {
        return false;
    }
    let count = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
    if count > (bytes.len() - 4) / size_of::<TcpRow>() {
        return false;
    }
    (0..count).any(|index| {
        let row = unsafe {
            std::ptr::read_unaligned(
                bytes
                    .as_ptr()
                    .add(4 + index * size_of::<TcpRow>())
                    .cast::<TcpRow>(),
            )
        };
        row.pid == pid
            && u16::from_be(row.local_port as u16) == port
            && row.local_address == u32::from_ne_bytes([127, 0, 0, 1])
            && row.remote_address == 0
            && row.remote_port == 0
    })
}
fn owned_api_ready(pid: u32) -> bool {
    if !owns_listener(pid) {
        return false;
    }
    let Ok(response) = super::agent(2)
        .get(&format!("{OWNED_ENDPOINT}/api/version"))
        .call()
    else {
        return false;
    };
    let Ok(value) = super::response_json(response) else {
        return false;
    };
    serde_json::from_slice::<Value>(MANIFEST)
        .is_ok_and(|manifest| value["version"] == manifest["version"])
}
pub(super) fn owned_running() -> bool {
    OWNED
        .get()
        .and_then(|value| value.lock().ok())
        .is_some_and(|mut state| {
            state.as_mut().is_some_and(|owned| {
                owned.child.try_wait().ok() == Some(None) && owns_listener(owned.child.id())
            })
        })
}
pub(super) fn ensure_owned() -> Result<(), String> {
    let mut state = OWNED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "Local AI is unavailable.".to_string())?;
    if let Some(owned) = state.as_mut() {
        if owned
            .child
            .try_wait()
            .map_err(|_| "Could not check Yas’s AI runtime.")?
            .is_none()
        {
            if owned_api_ready(owned.child.id()) {
                return Ok(());
            }
            return Err(
                "Yas’s AI runtime is starting or its port is unavailable. Try again.".into(),
            );
        }
    }
    state.take();
    let probe = TcpListener::bind("127.0.0.1:11435").map_err(|_| {
        "Yas’s private AI port is already in use. Close the other listener and try again."
            .to_string()
    })?;
    drop(probe);
    let folder = verify_runtime()?;
    let home = runtime_home()?;
    let job = Job::new()?;
    let mut command = Command::new(folder.join("ollama.exe"));
    command
        .arg("serve")
        .creation_flags(0x08000000)
        .current_dir(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    for name in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("System32");
    let paths = [folder.clone(), folder.join("lib/ollama"), system];
    command
        .env(
            "PATH",
            std::env::join_paths(paths)
                .map_err(|_| "Could not prepare the private AI runtime path.")?,
        )
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &home)
        .env("APPDATA", home.join("roaming"))
        .env("TEMP", home.join("tmp"))
        .env("TMP", home.join("tmp"))
        .env("OLLAMA_HOST", "127.0.0.1:11435")
        .env("OLLAMA_MODELS", home.join("models"))
        .env("OLLAMA_NO_CLOUD", "1")
        .env("OLLAMA_KEEP_ALIVE", "0")
        .env("OLLAMA_NUM_PARALLEL", "1")
        .env("OLLAMA_MAX_LOADED_MODELS", "1")
        .env("OLLAMA_CONTEXT_LENGTH", "4096")
        .env("OLLAMA_LLM_LIBRARY", "cpu")
        .env("OLLAMA_VULKAN", "0");
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Yas’s bundled AI runtime.".to_string())?;
    if unsafe { AssignProcessToJobObject(job.handle(), HANDLE(child.as_raw_handle())) }.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Could not isolate Yas’s AI process. No AI request was sent.".into());
    }
    let mut owned = Owned {
        child,
        job: Some(job),
        verified_model: false,
    };
    let until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < until {
        if owned
            .child
            .try_wait()
            .map_err(|_| "Could not check Yas’s AI process.")?
            .is_some()
        {
            return Err("Yas’s bundled AI runtime could not start on this PC.".into());
        }
        if owned_api_ready(owned.child.id()) {
            *state = Some(owned);
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Dropping the exact-owned job terminates its process tree.
    Err("Yas’s local AI runtime did not become ready. Try again.".into())
}
pub fn shutdown() {
    if let Some(state) = OWNED.get() {
        if let Ok(mut state) = state.lock() {
            state.take();
        }
    }
}
fn verify_model_files(cancel: Option<&AtomicBool>) -> Result<(), String> {
    let models = runtime_home()?.join("models");
    // The pinned runtime stores the registry's original manifest bytes
    // here. Its legacy compatibility manifest adds a layer and has a different
    // serialization/digest; it is not the canonical identity we pinned.
    let manifest = models.join("manifests-v2/ollama.com/library/qwen3/0.6b");
    if file_sha(&manifest, cancel)? != MODEL_MANIFEST {
        return Err("The downloaded AI model manifest does not match this Yas release.".into());
    }
    for (digest, bytes) in MODEL_PARTS {
        let path = models.join("blobs").join(format!("sha256-{digest}"));
        plain_file(&path, bytes)?;
        if file_sha(&path, cancel)? != digest {
            return Err("The local AI model failed its checksum check. Run AI setup again.".into());
        }
    }
    Ok(())
}
pub(super) fn verify_owned_model(model: &str) -> Result<(), String> {
    if model != SETUP_MODEL {
        return Err("Yas’s bundled runtime only runs its verified small model.".into());
    }
    let verified = OWNED
        .get()
        .and_then(|s| s.lock().ok())
        .is_some_and(|s| s.as_ref().is_some_and(|o| o.verified_model));
    if !verified {
        verify_model_files(None)?;
        if let Some(state) = OWNED.get() {
            if let Ok(mut state) = state.lock() {
                if let Some(owned) = state.as_mut() {
                    owned.verified_model = true;
                }
            }
        }
    }
    Ok(())
}
fn pull_progress(item: &Value, parts: &mut BTreeMap<String, u64>) -> Result<SetupProgress, String> {
    if item.get("error").is_some() {
        return Err("The local model download failed. Check your connection and try again.".into());
    }
    if let Some(digest) = item["digest"].as_str() {
        let digest = digest
            .strip_prefix("sha256:")
            .ok_or("The model download returned an invalid digest.")?;
        let expected = MODEL_PARTS
            .iter()
            .find(|(known, _)| *known == digest)
            .map(|(_, n)| *n)
            .ok_or("The model download changed from the verified Yas version.")?;
        if item["total"]
            .as_u64()
            .is_some_and(|total| total != expected)
        {
            return Err("The model download length changed from the verified Yas version.".into());
        }
        let completed = item["completed"].as_u64().unwrap_or(0);
        if completed > expected {
            return Err("The model download exceeded its verified size.".into());
        }
        parts.insert(digest.to_owned(), completed);
    }
    let status = item["status"]
        .as_str()
        .unwrap_or("Downloading local model")
        .chars()
        .take(160)
        .collect();
    Ok(SetupProgress {
        status,
        completed: parts.values().sum(),
        total: MODEL_BYTES,
    })
}
/// Downloads only after the user presses Setup. Partial blobs are retained by
/// Ollama for its normal verified resume; no website text enters this request.
pub fn setup(cancel: &AtomicBool, progress: impl Fn(SetupProgress)) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Local AI setup cancelled.".into());
    }
    progress(SetupProgress {
        status: "Starting Yas’s local AI runtime".into(),
        completed: 0,
        total: MODEL_BYTES,
    });
    ensure_owned()?;
    if cancel.load(Ordering::Acquire) {
        shutdown();
        return Err("Local AI setup cancelled.".into());
    }
    let body = serde_json::to_vec(&json!({"model":SETUP_MODEL,"stream":true}))
        .map_err(|_| "Could not prepare local AI setup.".to_string())?;
    let result = (|| {
        let mut response = super::agent(3600)
            .post(&format!("{OWNED_ENDPOINT}/api/pull"))
            .header("Content-Type", "application/json")
            .send(body.as_slice())
            .map_err(super::runtime_error)?;
        let mut reader = BufReader::new(response.body_mut().as_reader().take(MAX_PULL_BYTES + 1));
        let mut total = 0u64;
        let mut parts = BTreeMap::new();
        let mut success = false;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("Local AI setup cancelled.".into());
            }
            let mut line = Vec::new();
            let count = reader.read_until(b'\n', &mut line).map_err(|_| {
                "The model download stopped. Try setup again to resume.".to_string()
            })?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > MAX_PULL_BYTES || line.len() > 64 * 1024 {
                return Err("The model download response exceeded its limit.".into());
            }
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let item: Value = serde_json::from_slice(&line)
                .map_err(|_| "The model download returned invalid progress.".to_string())?;
            progress(pull_progress(&item, &mut parts)?);
            if item["status"] == "success" {
                success = true;
                break;
            }
        }
        if !success {
            return Err("The model download did not finish. Try setup again to resume.".into());
        }
        progress(SetupProgress {
            status: "Verifying the downloaded model".into(),
            completed: MODEL_BYTES,
            total: MODEL_BYTES,
        });
        verify_model_files(Some(cancel))?;
        if let Some(state) = OWNED.get() {
            if let Ok(mut state) = state.lock() {
                if let Some(owned) = state.as_mut() {
                    owned.verified_model = true;
                }
            }
        }
        progress(SetupProgress {
            status: "Local AI is ready".into(),
            completed: MODEL_BYTES,
            total: MODEL_BYTES,
        });
        Ok(())
    })();
    if result.is_err() || cancel.load(Ordering::Acquire) {
        shutdown();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn listener_only_table_rejects_wrong_owners_addresses_ports_and_truncated_rows() {
        let mut bytes = 1u32.to_ne_bytes().to_vec();
        for field in [
            0u32,
            u32::from_ne_bytes([127, 0, 0, 1]),
            u16::to_be(11435) as u32,
            0,
            0,
            7001,
        ] {
            bytes.extend(field.to_ne_bytes());
        }
        assert!(listener_table_owns(&bytes, 7001, 11435));
        bytes[4..8].copy_from_slice(&6u32.to_ne_bytes());
        assert!(listener_table_owns(&bytes, 7001, 11435));
        assert!(!listener_table_owns(&bytes, 7002, 11435));
        assert!(!listener_table_owns(&bytes, 7001, 11436));
        assert!(!listener_table_owns(&bytes[..bytes.len() - 1], 7001, 11435));
        assert!(!listener_table_owns(&[], 7001, 11435));
        bytes[8..12].copy_from_slice(&0u32.to_ne_bytes());
        assert!(!listener_table_owns(&bytes, 7001, 11435));
    }
    #[test]
    fn real_listener_ownership_requires_listen_and_disappears_after_close() {
        let bound = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = bound.local_addr().unwrap().port();
        assert!(owns_listener_on(std::process::id(), port));
        assert!(!owns_listener_on(0, port));
        drop(bound);
        let until = Instant::now() + Duration::from_secs(2);
        while owns_listener_on(std::process::id(), port) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!owns_listener_on(std::process::id(), port));
    }
    #[test]
    fn bundled_runtime_hashes_match_compiled_manifest_without_execution() {
        let source = Path::new(file!());
        let payload = source
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("ai-runtime");
        verify_runtime_at(&payload).unwrap();
    }
    #[test]
    fn runtime_manifest_is_pinned_cpu_only_and_has_safe_unique_paths() {
        let m: Value = serde_json::from_slice(MANIFEST).unwrap();
        assert_eq!(m["cpuOnly"], true);
        let mut names = HashSet::new();
        for file in m["files"].as_array().unwrap() {
            let p = file["path"].as_str().unwrap();
            assert!(safe_relative(p));
            assert!(names.insert(p.to_ascii_lowercase()));
            assert!(!p.contains("cuda") && !p.contains("vulkan"));
        }
        assert!(!safe_relative("../outside.exe"));
        assert!(!safe_relative("C:/outside.exe"));
        assert!(!safe_relative("\\outside.dll"));
    }
    #[test]
    fn model_download_progress_is_bounded_to_pinned_layers() {
        let mut parts = BTreeMap::new();
        let (digest, total) = MODEL_PARTS[0];
        let p = pull_progress(
            &json!({"digest":format!("sha256:{digest}"),"total":total,"completed":10}),
            &mut parts,
        )
        .unwrap();
        assert_eq!(p.completed, 10);
        assert_eq!(p.total, MODEL_BYTES);
        assert!(pull_progress(&json!({"digest":"sha256:changed","total":1}), &mut parts).is_err());
        assert!(pull_progress(
            &json!({"digest":format!("sha256:{digest}"),"total":total,"completed":total+1}),
            &mut parts
        )
        .is_err());
        assert_eq!(MODEL_PARTS.iter().map(|(_, n)| n).sum::<u64>(), MODEL_BYTES);
    }
}
