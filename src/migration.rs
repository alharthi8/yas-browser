//! Explicit migration of exported browser data. Never reads encrypted source-browser secrets.
#[cfg(test)]
use crate::storage::Vault;
use crate::storage::{self, Address, Entry, FormEntry, Settings};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::PathBuf;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Export {
    source: String,
    bookmarks: Vec<Entry>,
    history: Vec<Entry>,
    addresses: Vec<Address>,
    form_entries: Vec<FormEntry>,
    open_tabs: Vec<Entry>,
    reading_list: Vec<Entry>,
}

#[derive(Default, Serialize)]
pub struct Report {
    pub source: String,
    pub bookmarks_added: usize,
    pub bookmarks_updated: usize,
    pub history_added: usize,
    pub history_updated: usize,
    pub addresses_added: usize,
    pub form_entries_added: usize,
    pub form_entries_updated: usize,
    pub open_tabs_added: usize,
    pub reading_list_added: usize,
    pub password_rows_processed: usize,
    pub password_rows_skipped: usize,
    pub logins_added: usize,
    pub logins_updated: usize,
    pub skipped_entries: usize,
    pub backup_directory: Option<String>,
    pub unsupported: Vec<String>,
}

fn web_url(input: &str) -> Option<String> {
    let url = Url::parse(input.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.to_string())
}

fn merge_entries(
    target: &mut Vec<Entry>,
    incoming: Vec<Entry>,
    by_folder: bool,
) -> (usize, usize, usize) {
    let mut index: HashMap<(String, Option<String>), usize> = target
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            web_url(&e.url).map(|url| {
                (
                    (
                        url,
                        if by_folder {
                            e.folder
                                .clone()
                                .map(|f| f.trim().to_owned())
                                .filter(|f| !f.is_empty())
                        } else {
                            None
                        },
                    ),
                    i,
                )
            })
        })
        .collect();
    let (mut added, mut updated, mut skipped) = (0, 0, 0);
    for mut entry in incoming {
        let Some(url) = web_url(&entry.url) else {
            skipped += 1;
            continue;
        };
        entry.url = url.clone();
        entry.title = entry.title.trim().to_owned();
        if entry.title.is_empty() {
            entry.title = url.clone();
        }
        entry.folder = entry
            .folder
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty());
        entry.visited_at = entry.visited_at.filter(|t| t.is_finite() && *t >= 0.);
        let key = (
            url,
            if by_folder {
                entry.folder.clone()
            } else {
                None
            },
        );
        if let Some(&i) = index.get(&key) {
            let old = &mut target[i];
            let before = serde_json::to_value(&*old).unwrap();
            // Existing titles win; retain richer folder/time metadata from the export.
            if old.title.trim().is_empty() {
                old.title = entry.title;
            }
            if old.folder.is_none() {
                old.folder = entry.folder;
            }
            if entry
                .visited_at
                .is_some_and(|t| old.visited_at.is_none_or(|old| t > old))
            {
                old.visited_at = entry.visited_at;
            }
            if serde_json::to_value(&*old).unwrap() != before {
                updated += 1;
            }
        } else {
            index.insert(key, target.len());
            target.push(entry);
            added += 1;
        }
    }
    (added, updated, skipped)
}

fn normalize_address(mut a: Address) -> Address {
    for value in [
        &mut a.name,
        &mut a.email,
        &mut a.phone,
        &mut a.street,
        &mut a.city,
        &mut a.region,
        &mut a.postal,
        &mut a.country,
    ] {
        *value = value.trim().to_owned();
    }
    a
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
fn restore(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    // A failed atomic write usually leaves the original untouched; avoid rewriting it.
    if read_optional(path)?.as_deref() == bytes {
        return Ok(());
    }
    match bytes {
        Some(bytes) => storage::atomic_write(path, bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

/// Merge an explicit JSON export and optional locally exported password CSV into Yas.
/// Both target files are backed up before writes; a partial write is rolled back.
pub fn import(root: &Path, input: &Path, password_csv: Option<&Path>) -> Result<Report, String> {
    let export: Export =
        serde_json::from_slice(&fs::read(input).map_err(|e| format!("Cannot read export: {e}"))?)
            .map_err(|e| format!("Invalid migration JSON: {e}"))?;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let settings_path = root.join("settings.json");
    let vault_path = root.join("vault.dpapi");
    let original_settings = read_optional(&settings_path)?;
    let original_vault = read_optional(&vault_path)?;
    // Do not silently replace a damaged destination profile with defaults.
    let mut settings: Settings = match &original_settings {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|e| format!("Existing settings could not be read: {e}"))?,
        None => Settings::default(),
    };
    let mut vault = storage::read_vault(root)?;
    let settings_before = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    let vault_before = serde_json::to_value(&vault).map_err(|e| e.to_string())?;
    let mut report = Report {
        source: if export.source.trim().is_empty() {
            "Browser export".into()
        } else {
            export.source.chars().take(80).collect()
        },
        ..Default::default()
    };
    let (a, u, s) = merge_entries(&mut settings.bookmarks, export.bookmarks, true);
    report.bookmarks_added = a;
    report.bookmarks_updated = u;
    report.skipped_entries += s;
    let (a, u, s) = merge_entries(&mut settings.history, export.history, false);
    report.history_added = a;
    report.history_updated = u;
    report.skipped_entries += s;
    // Stable sort preserves the previous order of entries that have no exported timestamp.
    settings.history.sort_by(|a, b| {
        a.visited_at
            .unwrap_or(0.)
            .total_cmp(&b.visited_at.unwrap_or(0.))
    });
    let (a, _, s) = merge_entries(&mut settings.imported_tabs, export.open_tabs, false);
    report.open_tabs_added = a;
    report.skipped_entries += s;
    let (a, _, s) = merge_entries(&mut settings.reading_list, export.reading_list, false);
    report.reading_list_added = a;
    report.skipped_entries += s;
    if vault.address != Address::default() && !vault.addresses.contains(&vault.address) {
        vault.addresses.insert(0, vault.address.clone());
    }
    for address in export.addresses {
        let address = normalize_address(address);
        if address == Address::default() {
            report.skipped_entries += 1;
            continue;
        }
        if !vault.addresses.contains(&address) {
            vault.addresses.push(address.clone());
            report.addresses_added += 1;
        }
        if vault.address == Address::default() {
            vault.address = address;
        }
    }
    // General form-history values can contain personal information. They remain
    // inside the DPAPI-encrypted vault and are never included in the report.
    for mut entry in export.form_entries {
        if entry.value.is_empty() {
            report.skipped_entries += 1;
            continue;
        }
        entry.count = entry.count.max(0);
        if let Some(existing) = vault
            .form_entries
            .iter_mut()
            .find(|e| e.name == entry.name && e.value == entry.value)
        {
            if entry.count > existing.count {
                existing.count = entry.count;
                report.form_entries_updated += 1;
            }
        } else {
            vault.form_entries.push(entry);
            report.form_entries_added += 1;
        }
    }
    if let Some(csv) = password_csv {
        let old: HashMap<_, _> = vault
            .logins
            .iter()
            .map(|l| ((l.origin.clone(), l.username.clone()), l.password.clone()))
            .collect();
        report.password_rows_processed = storage::import_logins(csv.to_path_buf(), &mut vault)?;
        let mut rows = csv::Reader::from_path(csv).map_err(|e| e.to_string())?;
        let total_rows = rows.records().count();
        report.password_rows_skipped = total_rows.saturating_sub(report.password_rows_processed);
        if report.password_rows_skipped > 0 {
            report.unsupported.push("Some exported password rows were skipped because they used an unsupported credential scheme or had empty passwords.".into());
        }
        for login in &vault.logins {
            match old.get(&(login.origin.clone(), login.username.clone())) {
                None => report.logins_added += 1,
                Some(p) if *p != login.password => report.logins_updated += 1,
                _ => (),
            }
        }
    } else {
        report.unsupported.push("Passwords require a local CSV export; encrypted source-browser password databases are not read.".into());
    }
    report.unsupported.extend([
        "Payment cards and signed-in website sessions are not migrated.".into(),
        "Browser extensions require separate installation and compatibility checks.".into(),
    ]);
    let settings_changed =
        serde_json::to_value(&settings).map_err(|e| e.to_string())? != settings_before;
    let vault_changed = serde_json::to_value(&vault).map_err(|e| e.to_string())? != vault_before;
    if !settings_changed && !vault_changed {
        return Ok(report);
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let backup = root
        .join("import-backups")
        .join(format!("migration-{stamp}"));
    fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
    if let Some(bytes) = &original_settings {
        fs::write(backup.join("settings.json"), bytes).map_err(|e| e.to_string())?;
    }
    if let Some(bytes) = &original_vault {
        fs::write(backup.join("vault.dpapi"), bytes).map_err(|e| e.to_string())?;
    }
    report.backup_directory = Some(backup.to_string_lossy().into_owned());
    let commit = (|| {
        if vault_changed {
            storage::write_vault(root, &vault)?;
        }
        if settings_changed {
            storage::write_settings(root, &settings)?;
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = commit {
        let settings_restore = restore(&settings_path, original_settings.as_deref());
        let vault_restore = restore(&vault_path, original_vault.as_deref());
        if settings_restore.is_err() || vault_restore.is_err() {
            return Err(format!("Import failed: {error}. Automatic rollback was incomplete; original files remain in {}",backup.display()));
        }
        return Err(format!(
            "Import failed and original Yas data was restored: {error}"
        ));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "yas-migration-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn cleanup(root: PathBuf) {
        let absolute = fs::canonicalize(&root).unwrap();
        let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(
            absolute.starts_with(temp)
                && root
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("yas-migration-")
        );
        fs::remove_dir_all(absolute).unwrap();
    }
    #[test]
    fn merge_is_idempotent_preserves_existing_data_and_all_addresses() {
        let root = scratch("merge");
        let input = root.join("export.json");
        let mut settings = Settings::default();
        settings.bookmarks.push(Entry {
            title: "My title".into(),
            url: "https://example.com/".into(),
            ..Default::default()
        });
        storage::write_settings(&root, &settings).unwrap();
        fs::write(&input,r#"{"source":"Tabbit","bookmarks":[{"title":"Source title","url":"https://example.com","folder":"Work"},{"title":"New","url":"https://new.example/"},{"url":"javascript:alert(1)"}],"history":[{"url":"https://history.example/","visited_at":12345}],"addresses":[{"name":"One","city":"A"},{"name":"Two","city":"B"}],"open_tabs":[{"url":"https://tab.example/"}],"reading_list":[{"url":"https://read.example/"}]}"#).unwrap();
        let report = import(&root, &input, None).unwrap();
        assert_eq!(report.bookmarks_added, 2);
        assert_eq!(report.bookmarks_updated, 0);
        assert_eq!(report.skipped_entries, 1);
        assert_eq!(report.addresses_added, 2);
        assert!(report.backup_directory.is_some());
        let settings = storage::read_settings(&root).unwrap();
        assert_eq!(settings.bookmarks[0].title, "My title");
        assert_eq!(settings.bookmarks[0].folder.as_deref(), None);
        assert_eq!(settings.bookmarks[1].folder.as_deref(), Some("Work"));
        assert_eq!(settings.history[0].visited_at, Some(12345.));
        assert_eq!(settings.imported_tabs.len(), 1);
        assert_eq!(settings.reading_list.len(), 1);
        let vault = storage::read_vault(&root).unwrap();
        assert_eq!(vault.addresses.len(), 2);
        assert_eq!(vault.address.name, "One");
        let raw = fs::read(root.join("vault.dpapi")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("\"name\":\"One\""));
        let again = import(&root, &input, None).unwrap();
        assert_eq!(again.bookmarks_added, 0);
        assert_eq!(again.addresses_added, 0);
        assert!(again.backup_directory.is_none());
        cleanup(root);
    }
    #[test]
    fn malformed_password_csv_does_not_mutate_destination() {
        let root = scratch("bad-csv");
        let settings = root.join("settings.json");
        storage::write_settings(&root, &Settings::default()).unwrap();
        let before = fs::read(&settings).unwrap();
        let input = root.join("export.json");
        fs::write(&input, r#"{"bookmarks":[{"url":"https://new.example/"}]}"#).unwrap();
        let csv = root.join("bad.csv");
        fs::write(&csv, "url,username\nhttps://example.com,user\n").unwrap();
        assert!(import(&root, &input, Some(&csv)).is_err());
        assert_eq!(fs::read(settings).unwrap(), before);
        assert!(!root.join("vault.dpapi").exists());
        cleanup(root);
    }
    #[test]
    fn legacy_settings_and_vault_schema_remain_readable() {
        let settings: Settings = serde_json::from_str(
            r#"{"bookmarks":[{"title":"Legacy","url":"https://example.com"}]}"#,
        )
        .unwrap();
        assert!(settings.bookmarks[0].folder.is_none());
        assert!(settings.imported_tabs.is_empty());
        let vault: Vault =
            serde_json::from_str(r#"{"logins":[],"address":{"name":"Legacy"}}"#).unwrap();
        assert_eq!(vault.address.name, "Legacy");
        assert!(vault.addresses.is_empty());
    }
    #[test]
    fn password_export_merges_into_encrypted_vault_and_keeps_backup() {
        let root = scratch("passwords");
        let input = root.join("export.json");
        let csv = root.join("passwords.csv");
        fs::write(&input, r#"{"source":"Tabbit"}"#).unwrap();
        let mut vault = Vault::default();
        vault.logins.push(storage::Login {
            origin: "https://existing.example".into(),
            username: "alice".into(),
            password: "old-test-value".into(),
            label: None,
        });
        storage::write_vault(&root, &vault).unwrap();
        let old_bytes = fs::read(root.join("vault.dpapi")).unwrap();
        fs::write(&csv,"url,username,password\nhttps://existing.example/login,alice,new-test-value\nhttps://new.example/login,bob,other-test-value\nhttp://legacy.example/login,legacy,not-imported\n").unwrap();
        let report = import(&root, &input, Some(&csv)).unwrap();
        assert_eq!(report.logins_added, 2);
        assert_eq!(report.logins_updated, 1);
        assert_eq!(report.password_rows_processed, 3);
        assert_eq!(report.password_rows_skipped, 0);
        let result = storage::read_vault(&root).unwrap();
        assert_eq!(result.logins.len(), 3);
        assert_eq!(
            result
                .logins
                .iter()
                .find(|l| l.username == "alice")
                .unwrap()
                .password,
            "new-test-value"
        );
        assert_eq!(
            fs::read(PathBuf::from(report.backup_directory.unwrap()).join("vault.dpapi")).unwrap(),
            old_bytes
        );
        let encrypted = fs::read(root.join("vault.dpapi")).unwrap();
        assert!(!String::from_utf8_lossy(&encrypted).contains("new-test-value"));
        cleanup(root);
    }
    #[cfg(windows)]
    #[test]
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "Windows-only test restores FILE_ATTRIBUTE_READONLY on its own fixture without changing ACLs."
    )]
    fn settings_write_failure_rolls_back_already_written_vault() {
        let root = scratch("rollback");
        let input = root.join("export.json");
        let settings = root.join("settings.json");
        let vault = root.join("vault.dpapi");
        storage::write_settings(&root, &Settings::default()).unwrap();
        storage::write_vault(&root, &Vault::default()).unwrap();
        let before_settings = fs::read(&settings).unwrap();
        let before_vault = fs::read(&vault).unwrap();
        fs::write(&input,r#"{"bookmarks":[{"url":"https://new.example/"}],"addresses":[{"name":"New address"}]}"#).unwrap();
        let mut permissions = fs::metadata(&settings).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&settings, permissions).unwrap();
        let result = import(&root, &input, None);
        let mut permissions = fs::metadata(&settings).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&settings, permissions).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&settings).unwrap(), before_settings);
        assert_eq!(fs::read(&vault).unwrap(), before_vault);
        cleanup(root);
    }
    #[test]
    fn general_form_history_is_preserved_encrypted_and_imports_idempotently() {
        let root = scratch("forms");
        let input = root.join("export.json");
        fs::write(&input,r#"{"form_entries":[{"name":"email","value":"synthetic@example.invalid","count":4},{"name":"email","value":"synthetic@example.invalid","count":9},{"name":"custom","value":"  preserve spaces  ","count":2},{"name":"empty","value":"","count":1}]}"#).unwrap();
        let report = import(&root, &input, None).unwrap();
        assert_eq!(report.form_entries_added, 2);
        assert_eq!(report.form_entries_updated, 1);
        assert_eq!(report.skipped_entries, 1);
        let vault = storage::read_vault(&root).unwrap();
        assert_eq!(vault.form_entries.len(), 2);
        assert_eq!(vault.form_entries[0].count, 9);
        assert_eq!(vault.form_entries[1].value, "  preserve spaces  ");
        let encrypted = fs::read(root.join("vault.dpapi")).unwrap();
        assert!(!String::from_utf8_lossy(&encrypted).contains("synthetic@example.invalid"));
        let again = import(&root, &input, None).unwrap();
        assert_eq!(again.form_entries_added, 0);
        assert_eq!(again.form_entries_updated, 0);
        assert!(again.backup_directory.is_none());
        cleanup(root);
    }
    #[test]
    fn bookmark_copies_in_distinct_folders_are_preserved() {
        let root = scratch("folders");
        let input = root.join("export.json");
        fs::write(&input,r#"{"bookmarks":[{"url":"https://same.example/","title":"Work copy","folder":"Work"},{"url":"https://same.example/","title":"Personal copy","folder":"Personal"},{"url":"https://same.example/","title":"Duplicate work copy","folder":"Work"}]}"#).unwrap();
        let report = import(&root, &input, None).unwrap();
        assert_eq!(report.bookmarks_added, 2);
        let settings = storage::read_settings(&root).unwrap();
        assert_eq!(settings.bookmarks.len(), 2);
        assert_eq!(settings.bookmarks[0].folder.as_deref(), Some("Work"));
        assert_eq!(settings.bookmarks[1].folder.as_deref(), Some("Personal"));
        assert_eq!(import(&root, &input, None).unwrap().bookmarks_added, 0);
        assert_eq!(storage::read_settings(&root).unwrap().bookmarks.len(), 2);
        cleanup(root);
    }
}
