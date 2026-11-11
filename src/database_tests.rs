use super::*;
use crate::storage::{Address, FormEntry, Login};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "yas-database-test-{}-{stamp}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let resolved = fs::canonicalize(&self.0).unwrap();
        let temp = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(
            resolved.starts_with(temp)
                && self
                    .0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("yas-database-test-")
        );
        fs::remove_dir_all(resolved).unwrap();
    }
}

fn value<T: Serialize>(item: &T) -> serde_json::Value {
    serde_json::to_value(item).unwrap()
}
fn entry(title: &str, url: &str, folder: Option<&str>, visited_at: Option<f64>) -> Entry {
    Entry {
        title: title.into(),
        url: url.into(),
        folder: folder.map(str::to_owned),
        visited_at,
    }
}
fn rich_state() -> (Settings, Vault) {
    let settings = Settings {
        bookmarks: vec![
            entry(
                "Work · عمل",
                "https://same.example/?q=1#part",
                Some("Work/بحث"),
                None,
            ),
            entry(
                "Personal copy",
                "https://same.example/?q=1#part",
                Some("Personal"),
                Some(42.5),
            ),
        ],
        history: vec![
            entry("Older", "https://history.example/a", None, Some(1000.25)),
            entry(
                "Later duplicate",
                "https://history.example/a",
                None,
                Some(2000.75),
            ),
        ],
        session: vec![
            "https://same.example/?q=1#part".into(),
            "yas://newtab".into(),
            "https://same.example/?q=1#part".into(),
        ],
        session_tabs: vec![
            entry(
                "First dashboard",
                "https://same.example/?q=1#part",
                None,
                None,
            ),
            entry("New tab", "yas://newtab", None, None),
            entry(
                "Second dashboard · ياس",
                "https://same.example/?q=1#part",
                None,
                None,
            ),
        ],
        exceptions: vec!["example.test".into(), "unchanged.example".into()],
        custom_filters: "||ads.example^\nexample.test##.sponsor".into(),
        imported_tabs: vec![entry(
            "Imported",
            "https://imported.example/",
            Some("Imported folder"),
            Some(11.0),
        )],
        reading_list: vec![entry("Read later", "https://reading.example/", None, None)],
        background_private_downloads: false,
        notification_permissions: [
            ("https://notify.example".into(), "allow".into()),
            ("https://blocked.example".into(), "block".into()),
        ]
        .into(),
        appearance: "dark".into(),
        search_engine: "duckduckgo".into(),
        local_ai_model: "synthetic-offline-model".into(),
        check_updates: false,
    };
    let address = Address {
        name: "PRIVATE-ADDRESS-MARKER-049-نموذج".into(),
        email: "private-address-049@example.invalid".into(),
        phone: "+971000000000".into(),
        street: "Synthetic street".into(),
        city: "Dubai".into(),
        region: "Dubai".into(),
        postal: "00000".into(),
        country: "AE".into(),
    };
    let vault = Vault {
        logins: vec![
            Login {
                origin: "https://login.example".into(),
                username: "synthetic-user".into(),
                password: "PRIVATE-PASSWORD-MARKER-049-αβγ".into(),
                label: Some("Work login · حساب".into()),
            },
            Login {
                origin: "http://legacy.example".into(),
                username: "legacy".into(),
                password: "LEGACY-PASSWORD-MARKER-049".into(),
                label: None,
            },
            Login {
                origin: "android://hash@com.example.synthetic/".into(),
                username: "mobile".into(),
                password: "ANDROID-PASSWORD-MARKER-049".into(),
                label: Some("Mobile".into()),
            },
        ],
        address: address.clone(),
        addresses: vec![
            address,
            Address {
                name: "Second address".into(),
                city: "Abu Dhabi".into(),
                ..Address::default()
            },
        ],
        form_entries: vec![FormEntry {
            name: "contact".into(),
            value: "PRIVATE-FORM-MARKER-049 · حفظ المسافات  ".into(),
            count: 9,
        }],
    };
    (settings, vault)
}
fn legacy_vault(vault: &Vault) -> Vec<u8> {
    let mut plain = serde_json::to_vec(vault).unwrap();
    let protected = storage::protect(&plain, false).unwrap();
    plain.fill(0);
    protected
}
fn legacy_downloads() -> Vec<u8> {
    let mut plain = serde_json::to_vec(&serde_json::json!({
        "version":1,"next_id":2,"records":[
            {"id":1,"url":"https://private-download-049.example/complete","path":"C:\\Yas-database-test\\complete.bin","status":"completed","received":7,"total":7,"message":null},
            {"id":2,"url":"https://private-download-049.example/paused","path":"C:\\Yas-database-test\\paused.bin","status":"paused","received":4,"total":10,"message":"Synthetic paused record"}
        ]
    })).unwrap();
    let protected = storage::protect(&plain, false).unwrap();
    plain.fill(0);
    protected
}
fn seed(root: &Path, settings: &Settings, vault: &Vault) -> [Vec<u8>; 3] {
    let bytes = [
        serde_json::to_vec_pretty(settings).unwrap(),
        legacy_vault(vault),
        legacy_downloads(),
    ];
    for (name, bytes) in ["settings.json", "vault.dpapi", "downloads.dpapi"]
        .into_iter()
        .zip(&bytes)
    {
        fs::write(root.join(name), bytes).unwrap();
    }
    bytes
}
fn assert_legacy_unchanged(root: &Path, before: &[Vec<u8>; 3]) {
    for (name, expected) in ["settings.json", "vault.dpapi", "downloads.dpapi"]
        .into_iter()
        .zip(before)
    {
        assert_eq!(
            fs::read(root.join(name)).unwrap(),
            *expected,
            "Legacy input changed: {name}"
        );
    }
}
fn assert_no_database_or_stage(root: &Path) {
    assert!(!root.join(FILE_NAME).exists());
    assert!(!root.join(MARKER_NAME).exists());
    assert!(!fs::read_dir(root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".yas-db-staging-")));
}

#[test]
fn a_new_profile_has_binary_defaults_and_reopens_without_json_files() {
    let fixture = Fixture::new();
    let store = open_profile(&fixture.0).unwrap();
    assert_eq!(
        value(&read_settings_from(&store).unwrap()),
        value(&Settings::default())
    );
    assert_eq!(
        value(&read_vault_from(&store).unwrap()),
        value(&Vault::default())
    );
    assert!(read_value_from(&store, "downloads").unwrap().is_none());
    assert_eq!(fs::read(fixture.0.join(MARKER_NAME)).unwrap(), MARKER);
    for name in ["settings.json", "vault.dpapi", "downloads.dpapi"] {
        assert!(!fixture.0.join(name).exists());
    }
    drop(store);
    let mut reopened = builder().open(fixture.0.join(FILE_NAME)).unwrap();
    assert!(reopened.check_integrity().unwrap());
    assert_eq!(
        value(&read_settings_from(&reopened).unwrap()),
        value(&Settings::default())
    );
    reopened.close().unwrap();
}

#[test]
fn migration_preserves_rich_state_and_encrypts_private_values() {
    let fixture = Fixture::new();
    let (settings, vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    let store = open_profile(&fixture.0).unwrap();
    assert_eq!(
        value(&read_settings_from(&store).unwrap()),
        value(&settings)
    );
    assert_eq!(value(&read_vault_from(&store).unwrap()), value(&vault));
    let migrated_downloads = read_value_from(&store, "downloads").unwrap().unwrap();
    assert!(crate::downloads::ledgers_equal(&before[2], &migrated_downloads).unwrap());
    assert_legacy_unchanged(&fixture.0, &before);
    drop(store);
    let raw = fs::read(fixture.0.join(FILE_NAME)).unwrap();
    for marker in [
        "PRIVATE-PASSWORD-MARKER-049",
        "PRIVATE-ADDRESS-MARKER-049",
        "PRIVATE-FORM-MARKER-049",
        "https://private-download-049.example/",
    ] {
        assert!(
            !String::from_utf8_lossy(&raw).contains(marker),
            "Private value was written without encryption"
        );
    }
    let checked = verify_migration(&fixture.0).unwrap();
    assert_eq!(checked["passed"], true);
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn migrated_database_wins_over_stale_legacy_files_after_reopen() {
    let fixture = Fixture::new();
    let (mut settings, mut vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    let store = open_profile(&fixture.0).unwrap();
    settings.bookmarks.push(entry(
        "Added after migration",
        "https://new.example/",
        None,
        None,
    ));
    vault.logins[0].password = "CHANGED-IN-DATABASE-049".into();
    write_settings_and_optional_vault(&fixture.0, &settings, Some(&vault)).unwrap();
    drop(store);
    let reopened = open_profile(&fixture.0).unwrap();
    assert_eq!(
        value(&read_settings_from(&reopened).unwrap()),
        value(&settings)
    );
    assert_eq!(value(&read_vault_from(&reopened).unwrap()), value(&vault));
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn corrupt_present_settings_abort_before_database_publication() {
    let fixture = Fixture::new();
    let (_, vault) = rich_state();
    let invalid = b"{not valid settings";
    let encrypted = legacy_vault(&vault);
    fs::write(fixture.0.join("settings.json"), invalid).unwrap();
    fs::write(fixture.0.join("vault.dpapi"), &encrypted).unwrap();
    assert!(open_profile(&fixture.0).is_err());
    assert_no_database_or_stage(&fixture.0);
    assert_eq!(fs::read(fixture.0.join("settings.json")).unwrap(), invalid);
    assert_eq!(fs::read(fixture.0.join("vault.dpapi")).unwrap(), encrypted);
}

#[test]
fn corrupt_present_vault_never_becomes_an_empty_vault() {
    let fixture = Fixture::new();
    let (settings, _) = rich_state();
    let json = serde_json::to_vec(&settings).unwrap();
    let invalid = b"not a DPAPI vault";
    fs::write(fixture.0.join("settings.json"), &json).unwrap();
    fs::write(fixture.0.join("vault.dpapi"), invalid).unwrap();
    assert!(open_profile(&fixture.0).is_err());
    assert_no_database_or_stage(&fixture.0);
    assert_eq!(fs::read(fixture.0.join("settings.json")).unwrap(), json);
    assert_eq!(fs::read(fixture.0.join("vault.dpapi")).unwrap(), invalid);
}

#[test]
fn unsupported_present_download_ledger_prevents_partial_migration() {
    let fixture = Fixture::new();
    let (settings, vault) = rich_state();
    let mut before = seed(&fixture.0, &settings, &vault);
    before[2] = storage::protect(br#"{"version":999,"next_id":0,"records":[]}"#, false).unwrap();
    fs::write(fixture.0.join("downloads.dpapi"), &before[2]).unwrap();
    assert!(open_profile(&fixture.0).is_err());
    assert_no_database_or_stage(&fixture.0);
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn a_future_database_schema_never_falls_back_to_legacy_json() {
    let fixture = Fixture::new();
    let (settings, vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    drop(open_profile(&fixture.0).unwrap());
    let db = builder().open(fixture.0.join(FILE_NAME)).unwrap();
    let tx = db.begin_write().unwrap();
    {
        tx.open_table(META)
            .unwrap()
            .insert("version", SCHEMA_VERSION + 1)
            .unwrap();
    }
    tx.commit().unwrap();
    db.close().unwrap();
    assert!(open_profile(&fixture.0).is_err());
    let db = builder().open(fixture.0.join(FILE_NAME)).unwrap();
    {
        let tx = db.begin_read().unwrap();
        assert_eq!(
            tx.open_table(META)
                .unwrap()
                .get("version")
                .unwrap()
                .unwrap()
                .value(),
            SCHEMA_VERSION + 1
        );
    }
    db.close().unwrap();
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn a_missing_required_database_record_cannot_reimport_legacy_passwords() {
    let fixture = Fixture::new();
    let (settings, vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    drop(open_profile(&fixture.0).unwrap());
    let db = builder().open(fixture.0.join(FILE_NAME)).unwrap();
    let tx = db.begin_write().unwrap();
    {
        tx.open_table(VALUES).unwrap().remove("vault").unwrap();
    }
    tx.commit().unwrap();
    db.close().unwrap();
    assert!(open_profile(&fixture.0).is_err());
    let db = builder().open(fixture.0.join(FILE_NAME)).unwrap();
    assert!(read_value_from(&db, "vault").unwrap().is_none());
    db.close().unwrap();
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn missing_migrated_database_cannot_silently_revert_to_stale_sources() {
    let fixture = Fixture::new();
    let (mut settings, vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    let store = open_profile(&fixture.0).unwrap();
    settings
        .bookmarks
        .push(entry("Newer data", "https://newer.example/", None, None));
    write_settings_and_optional_vault(&fixture.0, &settings, None).unwrap();
    drop(store);
    fs::remove_file(fixture.0.join(FILE_NAME)).unwrap();
    let error = match open_profile(&fixture.0) {
        Ok(_) => panic!("Missing migrated DB must fail closed"),
        Err(error) => error,
    };
    assert!(error.contains("database is missing"));
    assert!(!fixture.0.join(FILE_NAME).exists());
    assert_eq!(fs::read(fixture.0.join(MARKER_NAME)).unwrap(), MARKER);
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn differential_saves_preserve_reordering_duplicates_and_trailing_deletions() {
    let fixture = Fixture::new();
    let (mut settings, vault) = rich_state();
    let store = open_profile(&fixture.0).unwrap();
    write_settings_and_optional_vault(&fixture.0, &settings, Some(&vault)).unwrap();
    settings.bookmarks.reverse();
    settings.bookmarks.push(entry(
        "Third same URL",
        &settings.bookmarks[0].url,
        None,
        None,
    ));
    settings.history.clear();
    settings.imported_tabs.clear();
    settings.reading_list.clear();
    settings.session.remove(1);
    settings.session_tabs.remove(1);
    write_settings_and_optional_vault(&fixture.0, &settings, None).unwrap();
    assert_eq!(
        value(&read_settings_from(&store).unwrap()),
        value(&settings)
    );
    assert_eq!(value(&read_vault_from(&store).unwrap()), value(&vault));
    let tx = store.begin_read().unwrap();
    let table = tx.open_table(ROWS).unwrap();
    assert_eq!(table.len().unwrap(), 7);
    assert!(table.get(("history", 0)).unwrap().is_none());
    assert!(table.get(("session", 2)).unwrap().is_none());
    assert!(table.get(("session_tabs", 2)).unwrap().is_none());
}

#[test]
fn explicit_legacy_backup_exports_current_database_values_and_encrypted_vault() {
    let fixture = Fixture::new();
    let (mut settings, mut vault) = rich_state();
    let before = seed(&fixture.0, &settings, &vault);
    let store = open_profile(&fixture.0).unwrap();
    settings.bookmarks.push(entry(
        "Current DB bookmark",
        "https://current.example/",
        None,
        None,
    ));
    vault.logins[0].password = "CURRENT-DATABASE-SECRET-MARKER-049".into();
    write_settings_and_optional_vault(&fixture.0, &settings, Some(&vault)).unwrap();
    let backup = fixture.0.join("private-export");
    storage::export_legacy_backup(&fixture.0, &backup).unwrap();
    let exported: Settings =
        serde_json::from_slice(&fs::read(backup.join("settings.json")).unwrap()).unwrap();
    assert_eq!(value(&exported), value(&settings));
    let encrypted = fs::read(backup.join("vault.dpapi")).unwrap();
    assert_eq!(
        value(&storage::decode_legacy_vault(Some(&encrypted)).unwrap()),
        value(&vault)
    );
    assert!(!String::from_utf8_lossy(&encrypted).contains("CURRENT-DATABASE-SECRET-MARKER-049"));
    assert!(crate::downloads::ledgers_equal(
        &fs::read(backup.join("downloads.dpapi")).unwrap(),
        &read_value_from(&store, "downloads").unwrap().unwrap()
    )
    .unwrap());
    assert_legacy_unchanged(&fixture.0, &before);
}

#[test]
fn staging_collision_does_not_delete_a_file_we_did_not_create() {
    let fixture = Fixture::new();
    let reserved = fixture.0.join(".yas-db-staging-collision.redb");
    let original = b"Owned by the test, not by the failed Stage creation";
    fs::write(&reserved, original).unwrap();
    assert!(Stage::create(reserved.clone()).is_err());
    assert_eq!(fs::read(&reserved).unwrap(), original);
    fs::remove_file(&reserved).unwrap();
    let stage = Stage::create(reserved.clone()).unwrap();
    assert!(reserved.exists());
    drop(stage);
    assert!(!reserved.exists());
}

#[test]
fn update_preference_keeps_the_original_binary_preferences_record_compatible() {
    let fixture = Fixture::new();
    let (settings, vault) = rich_state();
    seed(&fixture.0, &settings, &vault);
    let store = open_profile(&fixture.0).unwrap();
    assert!(!read_settings_from(&store).unwrap().check_updates);
    let original = read_value_from(&store, "preferences").unwrap().unwrap();
    let tx = store.begin_write().unwrap();
    tx.open_table(VALUES)
        .unwrap()
        .remove("check_updates_v1")
        .unwrap();
    tx.commit().unwrap();
    assert!(read_settings_from(&store).unwrap().check_updates);
    assert_eq!(
        original,
        read_value_from(&store, "preferences").unwrap().unwrap()
    );
    let _: PreferencesV1 = decode(&original).unwrap();
}
