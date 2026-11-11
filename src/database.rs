//! Versioned application records in redb. Website-engine profiles are separate.
//! Legacy files are read only during first migration and remain recovery copies.
use crate::storage::{self, Entry, Settings, Vault};
use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
    WriteTransaction,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock, Weak,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::HSTRING,
    Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH},
};

pub const FILE_NAME: &str = "yas.redb";
pub const MARKER_NAME: &str = "yas.storage-version";
pub const CACHE_BYTES: usize = 8 * 1024 * 1024;
const SCHEMA_VERSION: u64 = 1;
const MARKER: &[u8] = b"YAS-REDB-PROFILE\n1\n";
const META: TableDefinition<&str, u64> = TableDefinition::new("schema");
const COUNTS: TableDefinition<&str, u64> = TableDefinition::new("collection_lengths");
const ROWS: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("ordered_records_v1");
const VALUES: TableDefinition<&str, &[u8]> = TableDefinition::new("values_v1");
const COLLECTIONS: [&str; 6] = [
    "bookmarks",
    "history",
    "session",
    "session_tabs",
    "imported_tabs",
    "reading_list",
];
static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Database>>>> = OnceLock::new();
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub type ProfileStore = Arc<Database>;

#[derive(Serialize, Deserialize)]
struct PreferencesV1 {
    exceptions: Vec<String>,
    custom_filters: String,
    background_private_downloads: bool,
    notification_permissions: std::collections::BTreeMap<String, String>,
    appearance: String,
    search_engine: String,
    local_ai_model: String,
}

fn error(error: impl std::fmt::Display) -> String {
    format!("Could not access the Yas database: {error}. Existing database and recovery files were preserved.")
}
fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(value).map_err(error)
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let (value, rest) = postcard::take_from_bytes(bytes).map_err(error)?;
    if !rest.is_empty() {
        return Err(error("unexpected trailing record data"));
    }
    Ok(value)
}
fn entry_bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    // Explicit wire tuple: Entry's JSON skip attributes must not affect binary records.
    encode(&(&entry.title, &entry.url, &entry.folder, entry.visited_at))
}
fn entry_from_bytes(bytes: &[u8]) -> Result<Entry, String> {
    let (title, url, folder, visited_at) =
        decode::<(String, String, Option<String>, Option<f64>)>(bytes)?;
    Ok(Entry {
        title,
        url,
        folder,
        visited_at,
    })
}
fn preferences(settings: &Settings) -> PreferencesV1 {
    PreferencesV1 {
        exceptions: settings.exceptions.clone(),
        custom_filters: settings.custom_filters.clone(),
        background_private_downloads: settings.background_private_downloads,
        notification_permissions: settings.notification_permissions.clone(),
        appearance: settings.appearance.clone(),
        search_engine: settings.search_engine.clone(),
        local_ai_model: settings.local_ai_model.clone(),
    }
}
fn put_value(tx: &WriteTransaction, key: &str, value: &[u8]) -> Result<bool, String> {
    let mut table = tx.open_table(VALUES).map_err(error)?;
    let changed = table
        .get(key)
        .map_err(error)?
        .is_none_or(|old| old.value() != value);
    if changed {
        table.insert(key, value).map_err(error)?;
    }
    Ok(changed)
}
fn write_rows<T>(
    tx: &WriteTransaction,
    kind: &str,
    items: &[T],
    encode_item: impl Fn(&T) -> Result<Vec<u8>, String>,
) -> Result<bool, String> {
    let mut lengths = tx.open_table(COUNTS).map_err(error)?;
    let previous = lengths.get(kind).map_err(error)?.map(|v| v.value());
    let mut table = tx.open_table(ROWS).map_err(error)?;
    let mut changed = previous != Some(items.len() as u64);
    for (index, item) in items.iter().enumerate() {
        let bytes = encode_item(item)?;
        let key = (kind, index as u64);
        let different = table
            .get(key)
            .map_err(error)?
            .is_none_or(|old| old.value() != bytes);
        if different {
            table.insert(key, bytes.as_slice()).map_err(error)?;
            changed = true;
        }
    }
    for index in items.len() as u64..previous.unwrap_or(0) {
        table.remove((kind, index)).map_err(error)?;
    }
    if previous != Some(items.len() as u64) {
        lengths.insert(kind, items.len() as u64).map_err(error)?;
    }
    Ok(changed)
}
fn write_settings_tx(tx: &WriteTransaction, settings: &Settings) -> Result<bool, String> {
    let mut changed = put_value(tx, "preferences", &encode(&preferences(settings))?)?;
    changed |= write_rows(tx, "bookmarks", &settings.bookmarks, entry_bytes)?;
    changed |= write_rows(tx, "history", &settings.history, entry_bytes)?;
    changed |= write_rows(tx, "session", &settings.session, encode)?;
    changed |= write_rows(tx, "session_tabs", &settings.session_tabs, entry_bytes)?;
    changed |= write_rows(tx, "imported_tabs", &settings.imported_tabs, entry_bytes)?;
    changed |= write_rows(tx, "reading_list", &settings.reading_list, entry_bytes)?;
    Ok(changed)
}
fn validate_header(tx: &redb::ReadTransaction) -> Result<(), String> {
    let table = tx.open_table(META).map_err(error)?;
    if table.get("version").map_err(error)?.map(|v| v.value()) != Some(SCHEMA_VERSION)
        || table
            .get("migration_complete")
            .map_err(error)?
            .map(|v| v.value())
            != Some(1)
    {
        return Err(error("unsupported or incomplete application schema"));
    }
    let lengths = tx.open_table(COUNTS).map_err(error)?;
    if lengths.len().map_err(error)? != COLLECTIONS.len() as u64 {
        return Err(error("invalid collection schema"));
    }
    let mut total = 0u64;
    for name in COLLECTIONS {
        let length = lengths
            .get(name)
            .map_err(error)?
            .ok_or_else(|| error("missing collection"))?
            .value();
        total = total
            .checked_add(length)
            .ok_or_else(|| error("invalid collection length"))?;
    }
    if tx.open_table(ROWS).map_err(error)?.len().map_err(error)? != total {
        return Err(error("inconsistent record counts"));
    }
    Ok(())
}
fn read_rows<T>(
    tx: &redb::ReadTransaction,
    kind: &str,
    decode_item: impl Fn(&[u8]) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let length = tx
        .open_table(COUNTS)
        .map_err(error)?
        .get(kind)
        .map_err(error)?
        .ok_or_else(|| error("missing collection length"))?
        .value();
    let table = tx.open_table(ROWS).map_err(error)?;
    if length > table.len().map_err(error)? {
        return Err(error("invalid collection length"));
    }
    let mut items = Vec::new();
    for index in 0..length {
        let value = table
            .get((kind, index))
            .map_err(error)?
            .ok_or_else(|| error("missing ordered record"))?;
        items.push(decode_item(value.value())?);
    }
    Ok(items)
}
fn read_settings_from(db: &Database) -> Result<Settings, String> {
    let tx = db.begin_read().map_err(error)?;
    validate_header(&tx)?;
    let table = tx.open_table(VALUES).map_err(error)?;
    let value = table
        .get("preferences")
        .map_err(error)?
        .ok_or_else(|| error("missing preferences"))?;
    let prefs: PreferencesV1 = decode(value.value())?;
    Ok(Settings {
        bookmarks: read_rows(&tx, "bookmarks", entry_from_bytes)?,
        history: read_rows(&tx, "history", entry_from_bytes)?,
        session: read_rows(&tx, "session", decode)?,
        session_tabs: read_rows(&tx, "session_tabs", entry_from_bytes)?,
        imported_tabs: read_rows(&tx, "imported_tabs", entry_from_bytes)?,
        reading_list: read_rows(&tx, "reading_list", entry_from_bytes)?,
        exceptions: prefs.exceptions,
        custom_filters: prefs.custom_filters,
        background_private_downloads: prefs.background_private_downloads,
        notification_permissions: prefs.notification_permissions,
        appearance: prefs.appearance,
        search_engine: prefs.search_engine,
        local_ai_model: prefs.local_ai_model,
    })
}
fn read_value_from(db: &Database, key: &str) -> Result<Option<Vec<u8>>, String> {
    let tx = db.begin_read().map_err(error)?;
    validate_header(&tx)?;
    Ok(tx
        .open_table(VALUES)
        .map_err(error)?
        .get(key)
        .map_err(error)?
        .map(|v| v.value().to_vec()))
}
fn read_vault_from(db: &Database) -> Result<Vault, String> {
    let bytes = read_value_from(db, "vault")?.ok_or_else(|| error("missing encrypted vault"))?;
    storage::decode_vault_binary(&bytes)
}
fn builder() -> redb::Builder {
    let mut builder = Database::builder();
    builder
        .set_cache_size(CACHE_BYTES)
        .set_concurrency_mode(redb::ConcurrencyMode::ExclusiveWriter);
    builder
}
fn optional_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::metadata(path) {
        Ok(metadata)
            if path
                .file_name()
                .is_some_and(|name| name == "downloads.dpapi")
                && metadata.len() > 64 * 1024 * 1024 =>
        {
            return Err(
                "Saved download history is too large. The original file was preserved.".into(),
            );
        }
        Ok(metadata)
            if path.file_name().is_some_and(|name| name == MARKER_NAME) && metadata.len() > 64 =>
        {
            return Err(error("invalid storage marker"));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
        _ => {}
    }
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "Could not read {}: {e}. The existing file was preserved.",
            path.file_name().unwrap_or_default().to_string_lossy()
        )),
    }
}
struct Legacy {
    settings_bytes: Option<Vec<u8>>,
    vault_bytes: Option<Vec<u8>>,
    downloads_bytes: Option<Vec<u8>>,
    settings: Settings,
    vault: Vault,
    downloads: Option<Vec<u8>>,
}
impl Legacy {
    fn read(root: &Path) -> Result<Self, String> {
        let settings_bytes = optional_file(&root.join("settings.json"))?;
        let vault_bytes = optional_file(&root.join("vault.dpapi"))?;
        let downloads_bytes = optional_file(&root.join("downloads.dpapi"))?;
        let settings = storage::decode_legacy_settings(settings_bytes.as_deref())?;
        let vault = storage::decode_legacy_vault(vault_bytes.as_deref())?;
        let downloads = downloads_bytes
            .as_deref()
            .map(crate::downloads::migrate_legacy_ledger)
            .transpose()?;
        Ok(Self {
            settings_bytes,
            vault_bytes,
            downloads_bytes,
            settings,
            vault,
            downloads,
        })
    }
    fn inputs_unchanged(&self, root: &Path) -> Result<bool, String> {
        Ok(
            optional_file(&root.join("settings.json"))? == self.settings_bytes
                && optional_file(&root.join("vault.dpapi"))? == self.vault_bytes
                && optional_file(&root.join("downloads.dpapi"))? == self.downloads_bytes,
        )
    }
    fn equal_to(&self, db: &Database) -> Result<bool, String> {
        let downloads_equal = match (&self.downloads_bytes, read_value_from(db, "downloads")?) {
            (None, None) => true,
            (Some(old), Some(new)) => crate::downloads::ledgers_equal(old, &new)?,
            _ => false,
        };
        Ok(serde_json::to_value(&self.settings).map_err(error)?
            == serde_json::to_value(read_settings_from(db)?).map_err(error)?
            && serde_json::to_value(&self.vault).map_err(error)?
                == serde_json::to_value(read_vault_from(db)?).map_err(error)?
            && downloads_equal)
    }
}
struct Stage(Option<PathBuf>);
impl Stage {
    fn create(path: PathBuf) -> Result<Self, String> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(error)?;
        Ok(Self(Some(path)))
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
fn migrate(root: &Path, destination: &Path) -> Result<(), String> {
    let legacy = Legacy::read(root)?; // Validate every input before creating database files.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let stage_path = root.join(format!(
        ".yas-db-staging-{}-{stamp}-{}.redb",
        std::process::id(),
        STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut stage = Stage::create(stage_path.clone())?;
    {
        let db = builder().create(&stage_path).map_err(error)?;
        let mut tx = db.begin_write().map_err(error)?;
        tx.set_durability(Durability::Immediate).map_err(error)?;
        write_settings_tx(&tx, &legacy.settings)?;
        put_value(&tx, "vault", &storage::encode_vault_binary(&legacy.vault)?)?;
        if let Some(bytes) = &legacy.downloads {
            put_value(&tx, "downloads", bytes)?;
        }
        {
            let mut meta = tx.open_table(META).map_err(error)?;
            meta.insert("version", SCHEMA_VERSION).map_err(error)?;
            meta.insert("migration_complete", 1).map_err(error)?;
        }
        tx.commit().map_err(error)?;
        db.close().map_err(error)?;
    }
    {
        let mut db = builder().open(&stage_path).map_err(error)?;
        if !db.check_integrity().map_err(error)? || !legacy.equal_to(&db)? {
            return Err(error("migration readback verification failed"));
        }
        db.close().map_err(error)?;
    }
    if !legacy.inputs_unchanged(root)? {
        return Err(error("legacy data changed during migration"));
    }
    // No REPLACE_EXISTING flag: never overwrite a concurrently created database.
    unsafe {
        MoveFileExW(
            &HSTRING::from(stage_path.as_os_str()),
            &HSTRING::from(destination.as_os_str()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(error)?;
    stage.0.take();
    Ok(())
}
fn ensure_marker(root: &Path) -> Result<(), String> {
    let path = root.join(MARKER_NAME);
    if let Some(bytes) = optional_file(&path)? {
        if bytes != MARKER {
            return Err(error("unsupported storage marker"));
        }
        return Ok(());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_nanos();
    let stage_path = root.join(format!(
        ".yas-marker-staging-{}-{stamp}-{}",
        std::process::id(),
        STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut stage = Stage::create(stage_path.clone())?;
    {
        let mut file = OpenOptions::new()
            .write(true)
            .open(&stage_path)
            .map_err(error)?;
        file.write_all(MARKER)
            .and_then(|_| file.sync_all())
            .map_err(error)?;
    }
    match unsafe {
        MoveFileExW(
            &HSTRING::from(stage_path.as_os_str()),
            &HSTRING::from(path.as_os_str()),
            MOVEFILE_WRITE_THROUGH,
        )
    } {
        Ok(()) => {
            stage.0.take();
        }
        Err(e) => {
            if optional_file(&path)?.as_deref() != Some(MARKER) {
                return Err(error(e));
            }
        }
    }
    Ok(())
}
pub fn open_profile(root: &Path) -> Result<ProfileStore, String> {
    fs::create_dir_all(root).map_err(error)?;
    let key = fs::canonicalize(root).map_err(error)?;
    let mut cache = OPEN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| error("storage registry unavailable"))?;
    cache.retain(|_, weak| weak.strong_count() > 0);
    if let Some(db) = cache.get(&key).and_then(Weak::upgrade) {
        return Ok(db);
    }
    let path = key.join(FILE_NAME);
    let marker = optional_file(&key.join(MARKER_NAME))?;
    if marker.as_deref().is_some_and(|bytes| bytes != MARKER) {
        return Err(error("unsupported storage marker"));
    }
    if !path.try_exists().map_err(error)? {
        if marker.is_some() {
            return Err(error("the migrated database is missing; restore the verified database backup before starting Yas"));
        }
        migrate(&key, &path)?;
    }
    let db = builder().open(&path).map_err(error)?;
    // Existing invalid/future databases never fall back to stale JSON or empty defaults.
    read_settings_from(&db)?;
    read_vault_from(&db)?;
    if let Some(bytes) = read_value_from(&db, "downloads")? {
        crate::downloads::export_legacy_ledger(&bytes)?;
    }
    ensure_marker(&key)?;
    let db = Arc::new(db);
    cache.insert(key, Arc::downgrade(&db));
    Ok(db)
}
pub fn read_settings(root: &Path) -> Result<Settings, String> {
    let db = open_profile(root)?;
    read_settings_from(&db)
}
pub fn read_vault(root: &Path) -> Result<Vault, String> {
    let db = open_profile(root)?;
    read_vault_from(&db)
}
pub fn read_downloads(root: &Path) -> Result<Option<Vec<u8>>, String> {
    let db = open_profile(root)?;
    read_value_from(&db, "downloads")
}

#[cfg(test)]
thread_local! { static FAIL_TRANSACTION: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub fn fail_next_transaction_for_test(root: &Path) {
    FAIL_TRANSACTION.with(|slot| *slot.borrow_mut() = Some(fs::canonicalize(root).unwrap()));
}
pub fn write_settings_and_optional_vault(
    root: &Path,
    settings: &Settings,
    vault: Option<&Vault>,
) -> Result<(), String> {
    let db = open_profile(root)?;
    let mut tx = db.begin_write().map_err(error)?;
    tx.set_durability(Durability::Immediate).map_err(error)?;
    let mut changed = write_settings_tx(&tx, settings)?;
    if let Some(vault) = vault {
        changed |= put_value(&tx, "vault", &storage::encode_vault_binary(vault)?)?;
    }
    #[cfg(test)]
    if FAIL_TRANSACTION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|expected| fs::canonicalize(root).ok().as_ref() == Some(expected))
        {
            slot.take();
            true
        } else {
            false
        }
    }) {
        return Err("Injected test-only failure before transaction commit".into());
    }
    if changed {
        tx.commit().map_err(error)
    } else {
        tx.abort().map_err(error)
    }
}
pub fn write_vault(root: &Path, vault: &Vault) -> Result<(), String> {
    let db = open_profile(root)?;
    let mut tx = db.begin_write().map_err(error)?;
    tx.set_durability(Durability::Immediate).map_err(error)?;
    put_value(&tx, "vault", &storage::encode_vault_binary(vault)?)?;
    tx.commit().map_err(error)
}
pub fn write_downloads_if_unchanged(
    root: &Path,
    expected: Option<u64>,
    bytes: &[u8],
) -> Result<(), String> {
    let db = open_profile(root)?;
    let mut tx = db.begin_write().map_err(error)?;
    tx.set_durability(Durability::Immediate).map_err(error)?;
    {
        let table = tx.open_table(VALUES).map_err(error)?;
        let current = table.get("downloads").map_err(error)?;
        if current.as_ref().map(|v| storage::fingerprint(v.value())) != expected {
            return Err("Download history changed or was removed outside this manager. Existing records were preserved.".into());
        }
    }
    put_value(&tx, "downloads", bytes)?;
    tx.commit().map_err(error)
}

/// Trusted local maintenance command: compare full semantics without printing personal values.
pub fn verify_migration(root: &Path) -> Result<serde_json::Value, String> {
    let legacy = Legacy::read(root)?;
    {
        let store = open_profile(root)?;
        if !legacy.equal_to(&store)? {
            return Err(error("migrated records do not equal the legacy snapshot"));
        }
    }
    let mut db = builder().open(root.join(FILE_NAME)).map_err(error)?;
    let integrity = db.check_integrity().map_err(error)?;
    let settings = read_settings_from(&db)?;
    let vault = read_vault_from(&db)?;
    let settings_equal = serde_json::to_value(&settings).map_err(error)?
        == serde_json::to_value(&legacy.settings).map_err(error)?;
    let vault_equal = serde_json::to_value(&vault).map_err(error)?
        == serde_json::to_value(&legacy.vault).map_err(error)?;
    let downloads_equal = match (&legacy.downloads_bytes, read_value_from(&db, "downloads")?) {
        (None, None) => true,
        (Some(old), Some(new)) => crate::downloads::ledgers_equal(old, &new)?,
        _ => false,
    };
    let unchanged = legacy.inputs_unchanged(root)?;
    db.close().map_err(error)?;
    let passed = integrity && settings_equal && vault_equal && downloads_equal && unchanged;
    if !passed {
        return Err(error("storage migration verification failed"));
    }
    Ok(
        serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"mode":"storage-migration-verification","passed":true,"schemaVersion":SCHEMA_VERSION,"engine":"redb","cacheBytes":CACHE_BYTES,
        "checks":{"settingsEqual":settings_equal,"vaultEqual":vault_equal,"downloadHistoryEqual":downloads_equal,"legacyInputsUnchanged":unchanged,"databaseIntegrityPassed":integrity,"reopenVerified":true},
        "counts":{"bookmarks":settings.bookmarks.len(),"history":settings.history.len(),"savedTabs":settings.session.len(),"tabTitles":settings.session_tabs.len(),"logins":vault.logins.len(),"addresses":vault.addresses.len(),"formEntries":vault.form_entries.len()}}),
    )
}

#[cfg(test)]
#[path = "database_tests.rs"]
mod tests;
