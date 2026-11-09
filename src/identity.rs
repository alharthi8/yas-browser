//! Product identity compatibility. Old profile identifiers stay opaque so that
//! the renamed browser can continue using its existing cookies and credentials.
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::HSTRING,
    Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS},
};

pub const LEGACY_NEW_TAB: &str = "goodbro://newtab";
const REPARSE_POINT: u32 = 0x400;
static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn prepare_default_root() -> Result<PathBuf, String> {
    let current = directories::ProjectDirs::from("com", "Yas", "Yas")
        .ok_or("No local data directory")?
        .data_local_dir()
        .to_path_buf();
    let previous = directories::ProjectDirs::from("com", "GoodBro", "GoodBro")
        .ok_or("No local data directory")?
        .data_local_dir()
        .to_path_buf();
    migrate_legacy(&previous, &current)?;
    fs::create_dir_all(&current).map_err(|e| format!("Could not create the Yas profile: {e}"))?;
    Ok(current)
}

pub fn regular_profile(root: &Path) -> &'static str {
    if root.join("shell/EBWebView/WV2Profile_goodbroweb").is_dir() {
        "GoodBroWeb"
    } else {
        "YasWeb"
    }
}

pub fn lock_profile(root: &Path) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(root.join("instance.lock"))
        .map_err(|e| {
            if matches!(e.raw_os_error(), Some(32 | 33)) {
                "A browser using this profile is still open. Close it and try again.".into()
            } else {
                format!("Could not lock the browser profile: {e}")
            }
        })
}

fn checked_metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err("The previous profile contains a linked file or folder. Migration stopped without changing it.".into());
    }
    Ok(metadata)
}

#[expect(
    clippy::permissions_set_readonly_false,
    reason = "Windows-only migration: clears FILE_ATTRIBUTE_READONLY on our staging copies, without changing ACLs."
)]
fn copy_tree(source: &Path, destination: &Path, root: bool) -> Result<(), String> {
    let metadata = checked_metadata(source)?;
    if !metadata.is_dir() {
        return Err("The previous profile is not a directory.".into());
    }
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if root && entry.file_name() == "instance.lock" {
            continue;
        }
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let metadata = checked_metadata(&from)?;
        if metadata.is_dir() {
            fs::create_dir(&to).map_err(|e| e.to_string())?;
            copy_tree(&from, &to, false)?;
        } else if metadata.is_file() {
            fs::copy(&from, &to).map_err(|e| e.to_string())?;
            // Copies belong to our staging directory. Keeping them writable
            // also allows complete cleanup after a later copy failure.
            let mut permissions = fs::metadata(&to).map_err(|e| e.to_string())?.permissions();
            if permissions.readonly() {
                permissions.set_readonly(false);
                fs::set_permissions(&to, permissions).map_err(|e| e.to_string())?;
            }
        } else {
            return Err("The previous profile contains an unsupported file type.".into());
        }
    }
    Ok(())
}

struct StagingDirectory(Option<PathBuf>);
impl StagingDirectory {
    fn create(parent: &Path) -> Result<Self, String> {
        for _ in 0..16 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos();
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".yas-upgrade-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => return Ok(Self(Some(candidate))),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("Could not create a unique Yas migration staging directory.".into())
    }
    fn path(&self) -> &Path {
        self.0.as_deref().unwrap()
    }
    fn publish(mut self, destination: &Path) -> Result<(), String> {
        // No REPLACE_EXISTING flag: a concurrent Yas launch can never have its
        // profile overwritten, even if it creates the destination after recheck.
        unsafe {
            MoveFileExW(
                &HSTRING::from(self.path().as_os_str()),
                &HSTRING::from(destination.as_os_str()),
                MOVE_FILE_FLAGS(0),
            )
            .map_err(|e| format!("Could not publish the migrated Yas profile: {e}"))?;
        }
        self.0.take();
        Ok(())
    }
}
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            // This unique directory was created by us; source data is never removed.
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn verify_key_files(previous: &Path, staged: &Path) -> Result<(), String> {
    for name in ["settings.json", "vault.dpapi"] {
        let source = previous.join(name);
        match fs::read(&source) {
            Ok(before) => {
                let after = fs::read(staged.join(name)).map_err(|e| e.to_string())?;
                if before != after {
                    return Err("A profile file changed during migration. Close all browser copies and try again.".into());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if staged.join(name).try_exists().map_err(|e| e.to_string())? {
                    return Err("A profile file changed during migration. Try again.".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn resolve_parent_without_creating(parent: &Path) -> Result<PathBuf, String> {
    let mut missing = Vec::new();
    let mut ancestor = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    while !ancestor.try_exists().map_err(|e| e.to_string())? {
        missing.push(
            ancestor
                .file_name()
                .ok_or("Invalid Yas profile parent directory.")?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or("The Yas profile has no existing ancestor directory.")?;
        if ancestor.as_os_str().is_empty() {
            ancestor = Path::new(".");
        }
    }
    let mut resolved = fs::canonicalize(ancestor).map_err(|e| e.to_string())?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

/// Copy an existing legacy profile once, leaving the original intact. Existing
/// Yas data always wins. Only a fully copied profile is published at `new`.
pub fn migrate_legacy(old: &Path, new: &Path) -> Result<(), String> {
    if new.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    if !old.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    if !checked_metadata(old)?.is_dir() {
        return Err("The previous browser profile is not a directory.".into());
    }
    let _previous_lock = lock_profile(old)?;
    if new.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let parent = new
        .parent()
        .ok_or("The Yas profile has no parent directory.")?;
    let previous = fs::canonicalize(old).map_err(|e| e.to_string())?;
    let parent = resolve_parent_without_creating(parent)?;
    if parent.starts_with(&previous) {
        return Err("The new profile cannot be inside the previous profile.".into());
    }
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let name = new
        .file_name()
        .ok_or("The Yas profile has no directory name.")?;
    let destination = parent.join(name);
    let staging = StagingDirectory::create(&parent)?;
    copy_tree(&previous, staging.path(), true)?;
    verify_key_files(&previous, staging.path())?;
    if destination.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    staging.publish(&destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yas-identity-test-{}-{stamp}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn legacy(&self) -> PathBuf {
            let path = self.0.join("previous");
            fs::create_dir_all(path.join("shell/EBWebView/WV2Profile_goodbroweb/Network")).unwrap();
            fs::write(
                path.join("settings.json"),
                b"{\"session\":[\"goodbro://newtab\"]}",
            )
            .unwrap();
            fs::write(path.join("vault.dpapi"), [0, 1, 2, 255, 40, 50]).unwrap();
            fs::write(path.join("instance.lock"), []).unwrap();
            fs::write(
                path.join("shell/EBWebView/Local State"),
                b"profile-key-and-encrypted-key-fixture",
            )
            .unwrap();
            fs::write(
                path.join("shell/EBWebView/WV2Profile_goodbroweb/Network/Cookies"),
                b"cookie-database-fixture",
            )
            .unwrap();
            path
        }
        fn stages(&self) -> usize {
            fs::read_dir(&self.0)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with(".yas-upgrade-"))
                .count()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn complete_clone_preserves_data_and_uses_legacy_regular_profile() {
        let f = Fixture::new();
        let old = f.legacy();
        let new = f.0.join("current");
        migrate_legacy(&old, &new).unwrap();
        for name in [
            "settings.json",
            "vault.dpapi",
            "shell/EBWebView/Local State",
            "shell/EBWebView/WV2Profile_goodbroweb/Network/Cookies",
        ] {
            assert_eq!(
                fs::read(old.join(name)).unwrap(),
                fs::read(new.join(name)).unwrap()
            );
        }
        assert!(!new.join("instance.lock").exists());
        assert!(old.join("instance.lock").exists());
        assert_eq!(regular_profile(&new), "GoodBroWeb");
        assert_eq!(f.stages(), 0);
    }

    #[test]
    fn migration_is_once_only_and_existing_destination_is_never_merged() {
        let f = Fixture::new();
        let old = f.legacy();
        let new = f.0.join("current");
        migrate_legacy(&old, &new).unwrap();
        fs::write(new.join("settings.json"), b"new-browser-state").unwrap();
        fs::write(old.join("source-added-later"), b"not-imported").unwrap();
        let _busy = lock_profile(&old).unwrap();
        migrate_legacy(&old, &new).unwrap();
        assert_eq!(
            fs::read(new.join("settings.json")).unwrap(),
            b"new-browser-state"
        );
        assert!(!new.join("source-added-later").exists());
        assert_eq!(f.stages(), 0);
    }

    #[test]
    fn running_previous_browser_prevents_any_migration() {
        let f = Fixture::new();
        let old = f.legacy();
        let new = f.0.join("current");
        let _busy = lock_profile(&old).unwrap();
        assert!(migrate_legacy(&old, &new)
            .unwrap_err()
            .contains("still open"));
        assert!(!new.exists());
        assert!(!old.join("nested").exists());
        assert_eq!(f.stages(), 0);
    }

    #[test]
    fn copy_failure_cleans_only_own_stage_and_keeps_source() {
        let f = Fixture::new();
        let old = f.legacy();
        let new = f.0.join("current");
        let blocked = old.join("locked-fixture");
        fs::write(&blocked, b"untouched").unwrap();
        let _held = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&blocked)
            .unwrap();
        assert!(migrate_legacy(&old, &new).is_err());
        assert!(!new.exists());
        assert_eq!(
            fs::read(old.join("vault.dpapi")).unwrap(),
            [0, 1, 2, 255, 40, 50]
        );
        assert_eq!(f.stages(), 0);
    }

    #[test]
    fn profile_lock_excludes_second_writer_and_fresh_profiles_use_yas() {
        let f = Fixture::new();
        let _held = lock_profile(&f.0).unwrap();
        assert!(lock_profile(&f.0).is_err());
        assert_eq!(regular_profile(&f.0), "YasWeb");
    }

    #[test]
    fn rejects_destination_nested_in_source_without_publishing() {
        let f = Fixture::new();
        let old = f.legacy();
        let new = old.join("nested/current");
        assert!(migrate_legacy(&old, &new)
            .unwrap_err()
            .contains("cannot be inside"));
        assert!(!new.exists());
        assert!(!old.join("nested").exists());
        assert_eq!(f.stages(), 0);
    }
}
