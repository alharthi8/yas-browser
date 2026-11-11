use crate::update_protocol::{self as protocol, Release};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME, WAIT_OBJECT_0},
    System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    },
};

const NO_WINDOW: u32 = 0x08000000;
pub type CheckedRelease = (Release, Vec<u8>);

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn client() -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(600)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_body(Some(Duration::from_secs(15)))
        .build()
        .new_agent()
}

fn interrupted(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Update canceled.".into())
    } else {
        Ok(())
    }
}

fn cancellable<T>(
    client: ureq::Agent,
    cancel: &AtomicBool,
    request: impl FnOnce(&ureq::Agent) -> Result<T, String>,
) -> Result<T, String> {
    let finished = AtomicBool::new(false);
    struct Finish<'a>(&'a AtomicBool);
    impl Drop for Finish<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    std::thread::scope(|scope| {
        let interrupt = client.clone();
        let watcher = scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    interrupt.shutdown();
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let finish = Finish(&finished);
        let result = request(&client);
        drop(finish);
        let _ = watcher.join();
        interrupted(cancel)?;
        result
    })
}

fn trusted_redirect(url: &url::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("github.com") => {
            url.query().is_none()
                && url
                    .path()
                    .starts_with("/alharthi8/yas-browser/releases/download/")
        }
        Some("release-assets.githubusercontent.com") => true,
        _ => false,
    }
}

fn get(
    agent: &ureq::Agent,
    url: &str,
    cancel: &AtomicBool,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let mut current = url::Url::parse(url).map_err(|_| "Invalid update address.")?;
    for _ in 0..4 {
        interrupted(cancel)?;
        let response = agent
            .get(current.as_str())
            .call()
            .map_err(|error| match error {
                ureq::Error::StatusCode(404) => {
                    "Update files have not been published yet. Try again later.".to_string()
                }
                _ => "Could not reach GitHub updates. Check your connection and try again.".into(),
            })?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or("The update redirect was invalid.")?;
            let destination = current
                .join(location)
                .map_err(|_| "The update redirect was invalid.")?;
            if !trusted_redirect(&destination) {
                return Err(
                    "The update server redirected outside the trusted release hosts.".into(),
                );
            }
            current = destination;
        } else if response.status().as_u16() == 200 {
            return Ok(response);
        } else {
            return Err("The update server returned an unexpected response.".into());
        }
    }
    Err("The update server redirected too many times.".into())
}

fn signed_receipt(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "The signed update receipt is missing.")?
        .take(protocol::MAX_FEED + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The signed update receipt could not be read.")?;
    if bytes.len() as u64 > protocol::MAX_FEED {
        return Err("The signed update receipt is too large.".into());
    }
    Ok(bytes)
}

pub fn verify_feed_file(path: &Path) -> Result<Release, String> {
    protocol::verify(&signed_receipt(path)?, protocol::PUBLIC_KEY, now())
}

pub fn check(cancel: &AtomicBool) -> Result<Option<CheckedRelease>, String> {
    cancellable(client(), cancel, |agent| {
        interrupted(cancel)?;
        let mut response = get(agent, protocol::FEED_URL, cancel)?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(protocol::MAX_FEED)
            .read_to_vec()
            .map_err(|_| "The update feed was oversized or unreadable.")?;
        interrupted(cancel)?;
        let release = protocol::verify(&bytes, protocol::PUBLIC_KEY, now())?;
        if protocol::version(&release.version)? <= protocol::version(env!("CARGO_PKG_VERSION"))? {
            return Ok(None);
        }
        Ok(Some((release, bytes)))
    })
}

pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "Could not read the update file.")?;
    hash_reader(&mut file)
}

fn hash_reader(file: &mut impl Read) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Could not verify the update file.")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn safe_directory(path: &Path) -> Result<(), String> {
    if path.exists() {
        let metadata =
            fs::symlink_metadata(path).map_err(|_| "Could not inspect the update folder.")?;
        if !metadata.is_dir() || metadata.file_attributes() & 0x400 != 0 {
            return Err("The update folder must be a regular local directory.".into());
        }
    } else {
        fs::create_dir(path).map_err(|_| "Could not create the update folder.")?;
    }
    Ok(())
}

fn copy_payload(
    mut reader: impl Read,
    output: &mut File,
    release: &Release,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64, String> {
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    let mut hash = Sha256::new();
    let mut report_at = std::time::Instant::now();
    loop {
        interrupted(cancel)?;
        let count = reader
            .read(&mut buffer)
            .map_err(|_| "The update download was interrupted. Try again.")?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("Update size overflow.")?;
        if total > release.bytes {
            return Err("The update download exceeds its signed size.".into());
        }
        output
            .write_all(&buffer[..count])
            .map_err(|_| "Could not save the update. Check free disk space.")?;
        hash.update(&buffer[..count]);
        if report_at.elapsed() >= Duration::from_millis(200) {
            progress(total, release.bytes);
            report_at = std::time::Instant::now();
        }
    }
    interrupted(cancel)?;
    if total != release.bytes
        || !hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .eq_ignore_ascii_case(&release.sha256)
    {
        return Err("Update integrity verification failed. Nothing was installed.".into());
    }
    output
        .sync_all()
        .map_err(|_| "Could not finish saving the update.")?;
    progress(total, release.bytes);
    Ok(total)
}

pub fn download(
    root: &Path,
    release: &Release,
    envelope: &[u8],
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let verified = protocol::verify(envelope, protocol::PUBLIC_KEY, now())?;
    if serde_json::to_vec(&verified).ok() != serde_json::to_vec(release).ok() {
        return Err("The update selection changed. Check again.".into());
    }
    interrupted(cancel)?;
    let updates = root.join("updates");
    safe_directory(&updates)?;
    let folder = updates.join(format!(
        "{}-{}-{}",
        release.version,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    safe_directory(&folder)?;
    let partial = folder.join("installer.part");
    let result = cancellable(client(), cancel, |agent| {
        let mut response = get(agent, &release.url, cancel)?;
        if response
            .headers()
            .get("content-length")
            .is_some_and(|value| {
                value.to_str().ok().and_then(|s| s.parse::<u64>().ok()) != Some(release.bytes)
            })
        {
            return Err("The update download size does not match the signed release.".into());
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|_| "Could not create the update download.")?;
        let total = copy_payload(
            response.body_mut().as_reader(),
            &mut output,
            release,
            cancel,
            &mut progress,
        )?;
        drop(output);
        let installer = folder.join("installer.exe");
        fs::rename(&partial, &installer).map_err(|_| "Could not finalize the verified update.")?;
        let mut signed = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join("release.json"))
            .map_err(|_| "Could not retain the signed update receipt.")?;
        signed
            .write_all(envelope)
            .and_then(|_| signed.sync_all())
            .map_err(|_| "Could not retain the signed update receipt.")?;
        progress(total, release.bytes);
        Ok(installer)
    });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn install_directory() -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("Windows LocalAppData is unavailable.")?,
    )
    .join("Programs")
    .join("Yas"))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    }
}

fn creation_time(handle: windows::Win32::Foundation::HANDLE) -> Result<u64, String> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|_| "Could not verify the browser process identity.")?;
    Ok(((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64)
}

pub fn launch_handoff(root: &Path, installer: &Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|_| "Could not locate Yas Browser.")?;
    if !same_path(&current, &install_directory()?.join("Yas.exe")) {
        return Err("Restart-to-update is available in the installed browser. Install Yas Browser first; portable files are never overwritten automatically.".into());
    }
    let folder = installer.parent().ok_or("Invalid update staging folder.")?;
    let envelope = signed_receipt(&folder.join("release.json"))?;
    let release = protocol::verify(&envelope, protocol::PUBLIC_KEY, now())?;
    verify_installer(installer, &release)?;
    let helper = folder.join("YasUpdateHelper.exe");
    if !helper.exists() {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&helper)
            .map_err(|_| "Could not create the update handoff helper.")?;
        let mut input =
            File::open(&current).map_err(|_| "Could not read the current browser image.")?;
        std::io::copy(&mut input, &mut output)
            .and_then(|_| output.sync_all())
            .map_err(|_| "Could not save the update handoff helper.")?;
        drop(output);
    }
    if file_hash(&helper)? != file_hash(&current)? {
        return Err("The update handoff helper could not be verified.".into());
    }
    let created = creation_time(unsafe { GetCurrentProcess() })?;
    Command::new(&helper)
        .arg("--apply-verified-update")
        .arg(root)
        .arg(installer)
        .arg(std::process::id().to_string())
        .arg(created.to_string())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|_| "Windows could not start the verified update helper.")?;
    Ok(())
}

fn verify_installer(path: &Path, release: &Release) -> Result<File, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "The update installer is missing.")?;
    if !metadata.is_file()
        || metadata.file_attributes() & 0x400 != 0
        || metadata.len() != release.bytes
    {
        return Err("The update installer was changed.".into());
    }
    let mut locked = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| "The update installer is in use.")?;
    if locked
        .metadata()
        .map_err(|_| "Could not inspect the locked update file.")?
        .len()
        != release.bytes
    {
        return Err("The update installer was changed.".into());
    }
    if !hash_reader(&mut locked)?.eq_ignore_ascii_case(&release.sha256) {
        return Err("The update installer was changed.".into());
    }
    Ok(locked)
}

pub fn apply_handoff(args: &[String]) -> Result<bool, String> {
    let Some(index) = args.iter().position(|arg| arg == "--apply-verified-update") else {
        return Ok(false);
    };
    if args.len() != index + 5 {
        return Err("Invalid update handoff arguments.".into());
    }
    let root = PathBuf::from(&args[index + 1]);
    let installer = PathBuf::from(&args[index + 2]);
    let pid: u32 = args[index + 3]
        .parse()
        .map_err(|_| "Invalid update handoff process.")?;
    let created: u64 = args[index + 4]
        .parse()
        .map_err(|_| "Invalid update handoff process identity.")?;
    let folder = installer.parent().ok_or("Invalid update staging folder.")?;
    if !same_path(
        &root.join("updates"),
        folder.parent().ok_or("Invalid update staging folder.")?,
    ) || !same_path(
        &std::env::current_exe().map_err(|_| "Could not locate the update helper.")?,
        &folder.join("YasUpdateHelper.exe"),
    ) || installer.file_name().and_then(|s| s.to_str()) != Some("installer.exe")
    {
        return Err("The update handoff location is invalid.".into());
    }
    let bytes = signed_receipt(&folder.join("release.json"))?;
    let release = protocol::verify(&bytes, protocol::PUBLIC_KEY, now())?;
    if protocol::version(&release.version)? <= protocol::version(env!("CARGO_PKG_VERSION"))? {
        return Err("An update cannot downgrade or reinstall the current version.".into());
    }
    let installer_lock = verify_installer(&installer, &release)?;
    match unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    } {
        Ok(parent) => {
            let result = creation_time(parent).and_then(|actual| {
                if actual != created {
                    Err("The browser process identity changed.".into())
                } else if unsafe { WaitForSingleObject(parent, 30000) } != WAIT_OBJECT_0 {
                    Err("Yas Browser is still running. No update was installed.".into())
                } else {
                    Ok(())
                }
            });
            unsafe {
                let _ = CloseHandle(parent);
            }
            result?;
        }
        Err(error) if error.code() == windows::core::HRESULT(0x80070057u32 as i32) => {}
        Err(_) => {
            return Err("Could not verify that the browser exited. No update was installed.".into())
        }
    }
    let profile_lock = crate::identity::lock_profile(&root)?;
    backup_profile(&root)?;
    crate::database::verify_migration(&root)?;
    let directory = install_directory()?;
    let status = Command::new(&installer).args(["/VERYSILENT","/SUPPRESSMSGBOXES","/NORESTART","/SP-"])
        .arg(format!("/DIR={}",directory.display())).arg(format!("/LOG={}",folder.join("install.log").display()))
        .creation_flags(NO_WINDOW).status().map_err(|_| "Windows blocked or could not start the update installer. Your profile backup was preserved.")?;
    drop(installer_lock);
    drop(profile_lock);
    if !status.success() {
        return Err(
            "The update installer did not finish successfully. Your profile backup was preserved."
                .into(),
        );
    }
    let installed = directory.join("Yas.exe");
    let installed_version = Command::new(&installed)
        .arg("--yas-version")
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|_| "The installed update version could not be verified.")?;
    if !installed_version.status.success()
        || String::from_utf8_lossy(&installed_version.stdout).trim() != release.version
    {
        return Err("The installer did not install the expected version. Your profile backup was preserved.".into());
    }

    let verify = Command::new(&installed)
        .arg("--user-data-dir")
        .arg(&root)
        .arg("--verify-storage-migration")
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|_| {
            "The updated browser could not verify its profile. Your backup was preserved."
        })?;
    if !verify.status.success() {
        return Err("The updated browser did not pass its profile check. Your backup was preserved; no browsing window was opened.".into());
    }
    Command::new(&installed)
        .arg("--user-data-dir")
        .arg(root)
        .spawn()
        .map_err(|_| "The update was installed, but Windows could not reopen Yas Browser.")?;
    Ok(true)
}

fn backup_profile(root: &Path) -> Result<(), String> {
    let backups = root.join("update-backups");
    safe_directory(&backups)?;
    let destination = backups.join(format!("{}-{}", now(), std::process::id()));
    safe_directory(&destination)?;
    let mut manifest = serde_json::Map::new();
    for name in [
        "settings.json",
        "vault.dpapi",
        "downloads.dpapi",
        "yas.redb",
        "yas.storage-version",
    ] {
        let source = root.join(name);
        if !source.exists() {
            manifest.insert(name.into(), serde_json::Value::Null);
            continue;
        }
        let meta = fs::symlink_metadata(&source)
            .map_err(|_| "Could not inspect browser data before updating.")?;
        if !meta.is_file() || meta.file_attributes() & 0x400 != 0 {
            return Err("Browser data must be regular files before updating.".into());
        }
        let copy = destination.join(name);
        let mut input = File::open(&source).map_err(|_| "Could not back up browser data.")?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&copy)
            .map_err(|_| "Could not back up browser data.")?;
        std::io::copy(&mut input, &mut output)
            .and_then(|_| output.sync_all())
            .map_err(|_| "Could not finish the browser-data backup. No update was installed.")?;
        let hash = file_hash(&source)?;
        if hash != file_hash(&copy)? {
            return Err(
                "The browser-data backup could not be verified. No update was installed.".into(),
            );
        }
        manifest.insert(
            name.into(),
            serde_json::json!({"bytes":meta.len(),"sha256":hash}),
        );
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("backup-manifest.json"))
        .map_err(|_| "Could not retain the private backup receipt.")?;
    file.write_all(
        &serde_json::to_vec(&manifest).map_err(|_| "Could not encode the backup receipt.")?,
    )
    .and_then(|_| file.sync_all())
    .map_err(|_| "Could not finish the backup receipt.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "yas-update-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    fn cleanup(root: &Path) {
        assert_eq!(
            root.canonicalize().unwrap().parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("yas-update-test-"));
        fs::remove_dir_all(root).unwrap();
    }
    fn release() -> Release {
        Release {
            schema: 1,
            product: "yas-browser".into(),
            platform: "windows-x86_64".into(),
            version: "0.4.10".into(),
            url: String::new(),
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            bytes: 3,
            issued_at: 0,
            expires_at: 0,
            notes: String::new(),
        }
    }
    #[test]
    fn cancellation_interrupts_blocked_update_headers_and_body_reads() {
        use std::{
            net::TcpListener,
            sync::{mpsc, Arc},
        };
        for headers in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (ready_tx, ready_rx) = mpsc::channel();
            let server = std::thread::spawn(move || {
                let (mut peer, _) = listener.accept().unwrap();
                peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut request = [0u8; 4096];
                let _ = peer.read(&mut request);
                if headers {
                    peer.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nabc",
                    )
                    .unwrap();
                }
                ready_tx.send(()).unwrap();
                let _ = peer.read(&mut request);
            });
            let cancel = Arc::new(AtomicBool::new(false));
            let signal = cancel.clone();
            let trigger = std::thread::spawn(move || {
                ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                std::thread::sleep(Duration::from_millis(25));
                signal.store(true, Ordering::Release);
            });
            let agent = ureq::Agent::config_builder()
                .proxy(None)
                .timeout_global(Some(Duration::from_secs(2)))
                .build()
                .new_agent();
            let start = std::time::Instant::now();
            let result = cancellable(agent, &cancel, |client| {
                let mut response = client
                    .get(format!("http://{address}/"))
                    .call()
                    .map_err(|e| e.to_string())?;
                response
                    .body_mut()
                    .with_config()
                    .limit(2000)
                    .read_to_vec()
                    .map_err(|e| e.to_string())?;
                Ok(())
            });
            assert_eq!(result.unwrap_err(), "Update canceled.");
            assert!(start.elapsed() < Duration::from_secs(1));
            trigger.join().unwrap();
            server.join().unwrap();
        }
    }
    #[test]
    fn redirects_allow_only_github_release_and_asset_hosts() {
        for target in [
            "http://github.com/alharthi8/yas-browser/releases/download/v1/file",
            "https://github.com.evil.test/file",
            "https://github.com/another/repo/releases/download/file",
            "https://user:secret@release-assets.githubusercontent.com/file",
            "https://release-assets.githubusercontent.com:8443/file",
            "https://localhost/file",
            "https://github.com/alharthi8/yas-browser/releases/download/file?redirect=elsewhere",
        ] {
            assert!(
                !trusted_redirect(&url::Url::parse(target).unwrap()),
                "{target}"
            );
        }
        assert!(trusted_redirect(
            &url::Url::parse(
                "https://github.com/alharthi8/yas-browser/releases/download/v0.4.10/file"
            )
            .unwrap()
        ));
        assert!(trusted_redirect(
            &url::Url::parse(
                "https://release-assets.githubusercontent.com/file?temporary=public-download"
            )
            .unwrap()
        ));
    }
    #[test]
    fn streamed_download_checks_size_hash_cancellation_and_io_failure() {
        let root = fixture();
        let path = root.join("payload");
        let mut output = File::create(&path).unwrap();
        let cancel = AtomicBool::new(false);
        let mut progress = Vec::new();
        assert_eq!(
            copy_payload(&b"abc"[..], &mut output, &release(), &cancel, |a, b| {
                progress.push((a, b))
            })
            .unwrap(),
            3
        );
        assert_eq!(progress.last(), Some(&(3, 3)));
        assert_eq!(file_hash(&path).unwrap(), release().sha256);
        for data in [&b"ab"[..], &b"abcd"[..], &b"xyz"[..]] {
            let mut output = File::create(&path).unwrap();
            assert!(copy_payload(data, &mut output, &release(), &cancel, |_, _| {}).is_err());
        }
        cancel.store(true, Ordering::Release);
        let mut output = File::create(&path).unwrap();
        assert!(copy_payload(&b"abc"[..], &mut output, &release(), &cancel, |_, _| {}).is_err());
        assert_eq!(output.metadata().unwrap().len(), 0);
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("synthetic transfer failure"))
            }
        }
        cancel.store(false, Ordering::Release);
        assert!(copy_payload(Broken, &mut output, &release(), &cancel, |_, _| {}).is_err());
        drop(output);
        cleanup(&root);
    }
    #[test]
    fn verified_installer_is_locked_against_writes_and_rejects_tampering() {
        let root = fixture();
        let path = root.join("installer.exe");
        fs::write(&path, b"abc").unwrap();
        let locked = verify_installer(&path, &release()).unwrap();
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        drop(locked);
        fs::write(&path, b"xyz").unwrap();
        assert!(verify_installer(&path, &release()).is_err());
        fs::write(&path, b"ab").unwrap();
        assert!(verify_installer(&path, &release()).is_err());
        cleanup(&root);
    }
    #[test]
    fn private_backup_preserves_each_storage_file_byte_for_byte() {
        let root = fixture();
        for name in [
            "settings.json",
            "vault.dpapi",
            "downloads.dpapi",
            "yas.redb",
            "yas.storage-version",
        ] {
            fs::write(root.join(name), format!("synthetic opaque {name}")).unwrap();
        }
        backup_profile(&root).unwrap();
        let backup = fs::read_dir(root.join("update-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        for name in [
            "settings.json",
            "vault.dpapi",
            "downloads.dpapi",
            "yas.redb",
            "yas.storage-version",
        ] {
            assert_eq!(
                fs::read(root.join(name)).unwrap(),
                fs::read(backup.join(name)).unwrap()
            );
        }
        assert!(backup.join("backup-manifest.json").is_file());
        cleanup(&root);
    }
    #[test]
    fn unsigned_missing_and_oversized_receipts_and_invalid_handoffs_fail_closed() {
        let root = fixture();
        let receipt = root.join("release.json");
        assert!(signed_receipt(&receipt).is_err());
        fs::write(&receipt, vec![0; protocol::MAX_FEED as usize + 1]).unwrap();
        assert!(signed_receipt(&receipt).is_err());
        assert!(apply_handoff(&["Yas.exe".into(), "--apply-verified-update".into()]).is_err());
        assert!(!apply_handoff(&["Yas.exe".into()]).unwrap());
        let file = root.join("not-a-directory");
        fs::write(&file, b"probe").unwrap();
        assert!(safe_directory(&file).is_err());
        cleanup(&root);
    }
}
