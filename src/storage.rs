use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use url::Url;
use windows::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub title: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visited_at: Option<f64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub bookmarks: Vec<Entry>,
    pub history: Vec<Entry>,
    pub session: Vec<String>,
    pub session_tabs: Vec<Entry>,
    pub exceptions: Vec<String>,
    pub custom_filters: String,
    pub imported_tabs: Vec<Entry>,
    pub reading_list: Vec<Entry>,
    pub background_private_downloads: bool,
    pub notification_permissions: BTreeMap<String, String>,
    pub appearance: String,
    pub search_engine: String,
    pub local_ai_model: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bookmarks: vec![],
            history: vec![],
            session: vec![],
            session_tabs: vec![],
            exceptions: vec![],
            custom_filters: String::new(),
            imported_tabs: vec![],
            reading_list: vec![],
            background_private_downloads: false,
            notification_permissions: BTreeMap::new(),
            appearance: "system".into(),
            search_engine: "bing".into(),
            local_ai_model: "qwen3:0.6b".into(),
        }
    }
}

pub fn fallback_tab_title(url: &str) -> String {
    if url == "yas://newtab" || url == crate::identity::LEGACY_NEW_TAB {
        return "New tab".into();
    }
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}

fn saved_tab_title(title: &str) -> Option<&str> {
    let title = title.trim();
    (!title.is_empty() && !matches!(title, "Restored tab" | "New tab" | "about:blank"))
        .then_some(title)
}

impl Settings {
    pub fn set_session_tabs<'a>(&mut self, tabs: impl Iterator<Item = (&'a str, &'a str, bool)>) {
        self.session_tabs = tabs
            .filter(|(_, _, private)| !private)
            .map(|(url, title, _)| Entry {
                url: url.into(),
                title: saved_tab_title(title)
                    .map(str::to_owned)
                    .unwrap_or_else(|| fallback_tab_title(url)),
                ..Entry::default()
            })
            .collect();
        // Keep the URL array readable by earlier releases and import tools.
        self.session = self
            .session_tabs
            .iter()
            .map(|tab| tab.url.clone())
            .collect();
    }

    pub fn restored_session_tabs(&self) -> Vec<Entry> {
        self.session
            .iter()
            .enumerate()
            .map(|(index, url)| {
                // Position plus URL preserves distinct titles for duplicate URLs,
                // and ignores stale metadata if an older release changed the list.
                let saved = self
                    .session_tabs
                    .get(index)
                    .filter(|tab| tab.url == *url)
                    .and_then(|tab| saved_tab_title(&tab.title));
                let title = saved.or_else(|| {
                    self.history
                        .iter()
                        .rev()
                        .chain(&self.bookmarks)
                        .chain(&self.imported_tabs)
                        .chain(&self.reading_list)
                        .filter(|entry| entry.url == *url)
                        .find_map(|entry| saved_tab_title(&entry.title))
                });
                Entry {
                    url: url.clone(),
                    title: if destination(url) == "yas://newtab" {
                        "New tab".into()
                    } else {
                        title
                            .map(str::to_owned)
                            .unwrap_or_else(|| fallback_tab_title(url))
                    },
                    ..Entry::default()
                }
            })
            .collect()
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Login {
    pub origin: String,
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Address {
    pub name: String,
    pub email: String,
    pub phone: String,
    pub street: String,
    pub city: String,
    pub region: String,
    pub postal: String,
    pub country: String,
}
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormEntry {
    pub name: String,
    pub value: String,
    pub count: i64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Vault {
    pub logins: Vec<Login>,
    pub address: Address,
    pub addresses: Vec<Address>,
    pub form_entries: Vec<FormEntry>,
}

pub fn origin(input: &str) -> Option<String> {
    let u = Url::parse(input).ok()?;
    if u.scheme() != "https" || u.host_str().is_none() {
        return None;
    }
    Some(u.origin().ascii_serialization())
}
/// Storage scope for exported credentials. Fill policy deliberately remains
/// stricter: `origin` above accepts only matching HTTPS website origins.
pub fn login_scope(input: &str) -> Option<String> {
    let u = Url::parse(input.trim()).ok()?;
    u.host_str()?;
    match u.scheme() {
        "http" | "https" => Some(u.origin().ascii_serialization()),
        "android" if u.password().is_none() => Some(u.to_string()),
        _ => None,
    }
}
pub fn destination(input: &str) -> String {
    destination_with_engine(input, "bing")
}
pub fn destination_with_engine(input: &str, engine: &str) -> String {
    let s = input.trim();
    if s.is_empty() || s == "yas://newtab" || s == crate::identity::LEGACY_NEW_TAB {
        return "yas://newtab".into();
    }
    if let Ok(u) = Url::parse(s) {
        if matches!(u.scheme(), "http" | "https") {
            return u.to_string();
        }
    }
    if !s.contains(char::is_whitespace)
        && (s.contains('.') || s.starts_with("localhost") || s.starts_with("127.0.0.1"))
        && !s.contains("://")
    {
        let candidate = format!(
            "{}://{}",
            if s.starts_with("localhost") || s.starts_with("127.0.0.1") {
                "http"
            } else {
                "https"
            },
            s
        );
        if let Ok(u) = Url::parse(&candidate) {
            if u.host_str().is_some_and(|h| {
                h.contains('.') || h == "localhost" || h.parse::<std::net::IpAddr>().is_ok()
            }) {
                return u.to_string();
            }
        }
    }
    let base = match engine {
        "google" => "https://www.google.com/search",
        "duckduckgo" => "https://duckduckgo.com/",
        "brave" => "https://search.brave.com/search",
        _ => "https://www.bing.com/search",
    };
    let mut u = Url::parse(base).unwrap();
    u.query_pairs_mut().append_pair("q", s);
    u.into()
}
pub fn read_settings(root: &Path) -> Result<Settings, String> {
    crate::database::read_settings(root)
}
pub(crate) fn decode_legacy_settings(bytes: Option<&[u8]>) -> Result<Settings, String> {
    let Some(bytes) = bytes else {
        return Ok(Settings::default());
    };
    let mut settings: Settings = serde_json::from_slice(bytes).map_err(|error| {
        format!("Could not read settings.json: {error}. The existing file was preserved. Restore a valid settings backup before starting Yas.")
    })?;
    for url in &mut settings.session {
        if url == crate::identity::LEGACY_NEW_TAB {
            *url = "yas://newtab".into();
        }
    }
    Ok(settings)
}
pub fn write_settings(root: &Path, data: &Settings) -> Result<(), String> {
    crate::database::write_settings_and_optional_vault(root, data, None)
}
pub use crate::database::{open_profile, ProfileStore};
pub fn write_settings_and_vault(
    root: &Path,
    settings: &Settings,
    vault: &Vault,
) -> Result<(), String> {
    crate::database::write_settings_and_optional_vault(root, settings, Some(vault))
}
#[cfg(test)]
pub fn fail_next_transaction_for_test(root: &Path) {
    crate::database::fail_next_transaction_for_test(root);
}
pub fn read_downloads(root: &Path) -> Result<Option<Vec<u8>>, String> {
    crate::database::read_downloads(root)
}
pub fn write_downloads_if_unchanged(
    root: &Path,
    expected: Option<u64>,
    bytes: &[u8],
) -> Result<(), String> {
    crate::database::write_downloads_if_unchanged(root, expected, bytes)
}
pub fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    // Windows rename cannot replace an existing destination. ReplaceFileW preserves the original on failure.
    if path.exists() {
        use windows::{
            core::HSTRING,
            Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS},
        };
        unsafe {
            ReplaceFileW(
                &HSTRING::from(path.as_os_str()),
                &HSTRING::from(tmp.as_os_str()),
                None,
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
            .map_err(|e| e.to_string())
        }
    } else {
        fs::rename(tmp, path).map_err(|e| e.to_string())
    }
}
pub(crate) fn protect(bytes: &[u8], decrypt: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let result = if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        result.map_err(|e| e.to_string())?;
        let data = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(Some(HLOCAL(output.pbData as _)));
        Ok(data)
    }
}
pub fn read_vault(root: &Path) -> Result<Vault, String> {
    crate::database::read_vault(root)
}
pub fn write_vault(root: &Path, data: &Vault) -> Result<(), String> {
    crate::database::write_vault(root, data)
}
pub(crate) fn decode_legacy_vault(bytes: Option<&[u8]>) -> Result<Vault, String> {
    let Some(bytes) = bytes else {
        return Ok(Vault::default());
    };
    let mut plain = protect(bytes, true).map_err(|_| {
        "Could not decrypt the existing password vault. Original data was preserved.".to_string()
    })?;
    let parsed = serde_json::from_slice(&plain).map_err(|_| {
        "Could not read the existing password vault. Original data was preserved.".to_string()
    });
    plain.fill(0);
    parsed
}
pub(crate) fn encode_vault_binary(vault: &Vault) -> Result<Vec<u8>, String> {
    let logins: Vec<_> = vault
        .logins
        .iter()
        .map(|login| {
            (
                &login.origin,
                &login.username,
                &login.password,
                &login.label,
            )
        })
        .collect();
    let mut plain = postcard::to_allocvec(&(
        1u32,
        logins,
        &vault.address,
        &vault.addresses,
        &vault.form_entries,
    ))
    .map_err(|e| e.to_string())?;
    let encrypted = protect(&plain, false);
    plain.fill(0);
    encrypted
}
pub(crate) fn decode_vault_binary(bytes: &[u8]) -> Result<Vault, String> {
    type Wire = (
        u32,
        Vec<(String, String, String, Option<String>)>,
        Address,
        Vec<Address>,
        Vec<FormEntry>,
    );
    let mut plain = protect(bytes, true).map_err(|_| {
        "Could not decrypt the database password vault. Original data was preserved.".to_string()
    })?;
    let parsed =
        postcard::take_from_bytes::<Wire>(&plain).map(|(wire, rest)| (wire, rest.is_empty()));
    plain.fill(0);
    let ((version, logins, address, addresses, form_entries), complete) = parsed.map_err(|_| {
        "The database password vault is invalid. Original data was preserved.".to_string()
    })?;
    if version != 1 || !complete {
        return Err(
            "Unsupported database password-vault format. Original data was preserved.".into(),
        );
    }
    Ok(Vault {
        logins: logins
            .into_iter()
            .map(|(origin, username, password, label)| Login {
                origin,
                username,
                password,
                label,
            })
            .collect(),
        address,
        addresses,
        form_entries,
    })
}
pub fn export_legacy_backup(root: &Path, destination: &Path) -> Result<(), String> {
    if root == destination {
        return Err("Choose a separate private backup directory.".into());
    }
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    if fs::canonicalize(root).map_err(|e| e.to_string())?
        == fs::canonicalize(destination).map_err(|e| e.to_string())?
    {
        return Err("Choose a separate private backup directory.".into());
    }
    let settings = read_settings(root)?;
    let data = read_vault(root)?;
    write_new_export(
        &destination.join("settings.json"),
        &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )?;
    let mut plain = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    let encrypted = protect(&plain, false);
    plain.fill(0);
    write_new_export(&destination.join("vault.dpapi"), &encrypted?)?;
    if let Some(bytes) = read_downloads(root)? {
        write_new_export(
            &destination.join("downloads.dpapi"),
            &crate::downloads::export_legacy_ledger(&bytes)?,
        )?;
    }
    Ok(())
}
fn write_new_export(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
pub fn import_logins(path: PathBuf, vault: &mut Vault) -> Result<usize, String> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| e.to_string())?;
    let h = rdr.headers().map_err(|e| e.to_string())?.clone();
    let field = |name: &str| {
        h.iter()
            .position(|s| s.trim_start_matches('\u{feff}').eq_ignore_ascii_case(name))
            .ok_or(format!("Missing CSV column: {name}"))
    };
    let (u, n, p) = (field("url")?, field("username")?, field("password")?);
    let label_column = h.iter().position(|s| {
        s.trim_start_matches('\u{feff}')
            .eq_ignore_ascii_case("name")
    });
    let mut imported = Vec::new();
    for row in rdr.records() {
        let row = row.map_err(|e| e.to_string())?;
        if let Some(site) = row.get(u).and_then(login_scope) {
            let login = Login {
                origin: site,
                username: row.get(n).unwrap_or_default().into(),
                password: row.get(p).unwrap_or_default().into(),
                label: label_column
                    .and_then(|i| row.get(i))
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned),
            };
            if !login.password.is_empty() {
                imported.push(login);
            }
        }
    }
    let count = imported.len();
    for mut login in imported {
        if login.label.is_none() {
            login.label = vault
                .logins
                .iter()
                .find(|l| l.origin == login.origin && l.username == login.username)
                .and_then(|l| l.label.clone());
        }
        vault
            .logins
            .retain(|l| l.origin != login.origin || l.username != login.username);
        vault.logins.push(login);
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_sessions_recover_exact_url_titles_without_loading_pages() {
        let settings: Settings = serde_json::from_str(
            r#"{"session":["https://mail.example/inbox","https://saved.example/","https://unknown.example/a"],
            "history":[{"url":"https://mail.example/inbox","title":"Inbox · بريد"},
                       {"url":"https://mail.example/inbox?other","title":"Wrong page"},
                       {"url":"https://mail.example/inbox","title":"Restored tab"}],
            "bookmarks":[{"url":"https://saved.example/","title":"Saved research"}]}"#,
        ).unwrap();
        let tabs = settings.restored_session_tabs();
        assert_eq!(tabs[0].title, "Inbox · بريد");
        assert_eq!(tabs[1].title, "Saved research");
        assert_eq!(tabs[2].title, "unknown.example");
    }

    #[test]
    fn session_title_roundtrip_keeps_duplicate_urls_and_unicode_distinct() {
        let mut settings = Settings::default();
        settings.set_session_tabs(
            [
                ("https://same.example/", "First document · ياس", false),
                (
                    "https://same.example/",
                    "Second <document> & \"title\"",
                    false,
                ),
            ]
            .into_iter(),
        );
        let encoded = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&encoded).unwrap();
        let tabs = restored.restored_session_tabs();
        assert_eq!(restored.session.len(), 2);
        assert_eq!(tabs[0].title, "First document · ياس");
        assert_eq!(tabs[1].title, "Second <document> & \"title\"");
        assert_eq!(tabs[0].url, tabs[1].url);
    }

    #[test]
    fn private_session_titles_and_urls_are_excluded_in_the_same_pass() {
        let mut settings = Settings::default();
        settings.set_session_tabs(
            [
                ("https://same.example/", "Regular", false),
                ("https://same.example/", "PRIVATE-TITLE-MARKER", true),
                (
                    "https://private-marker.example/",
                    "PRIVATE-OTHER-MARKER",
                    true,
                ),
                ("https://last.example/", "Last", false),
            ]
            .into_iter(),
        );
        let encoded = serde_json::to_string(&settings).unwrap();
        assert!(!encoded.contains("PRIVATE-"));
        assert!(!encoded.contains("private-marker"));
        assert_eq!(
            settings.session,
            ["https://same.example/", "https://last.example/"]
        );
        assert_eq!(settings.session_tabs.len(), 2);
        assert_eq!(settings.restored_session_tabs()[1].title, "Last");
    }

    #[test]
    fn stale_session_metadata_cannot_rename_an_unrelated_or_reordered_tab() {
        let settings: Settings = serde_json::from_str(
            r#"{"session":["https://second.example/","https://first.example/"],
            "session_tabs":[{"url":"https://first.example/","title":"Wrong first slot"},
                            {"url":"https://second.example/","title":"Wrong second slot"}],
            "bookmarks":[{"url":"https://second.example/","title":"Second bookmark"}]}"#,
        )
        .unwrap();
        let tabs = settings.restored_session_tabs();
        assert_eq!(tabs[0].title, "Second bookmark");
        assert_eq!(tabs[1].title, "first.example");
    }

    #[test]
    fn updated_session_replaces_old_titles_and_keeps_legacy_url_array() {
        let mut settings = Settings::default();
        settings.set_session_tabs([("https://one.example/", "Before", false)].into_iter());
        settings.set_session_tabs(
            [
                ("https://two.example/", "Second", false),
                ("https://one.example/", "After", false),
            ]
            .into_iter(),
        );
        let encoded: serde_json::Value = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            encoded["session"],
            serde_json::json!(["https://two.example/", "https://one.example/"])
        );
        assert_eq!(settings.restored_session_tabs()[1].title, "After");
        settings.set_session_tabs(std::iter::empty());
        assert!(settings.session.is_empty() && settings.session_tabs.is_empty());
    }

    #[test]
    fn missing_placeholder_titles_and_new_tab_alias_have_clear_fallbacks() {
        let mut settings = Settings::default();
        settings.set_session_tabs(
            [
                ("https://example.test/path", "about:blank", false),
                ("https://other.test/", "  ", false),
                (crate::identity::LEGACY_NEW_TAB, "Old custom title", false),
            ]
            .into_iter(),
        );
        let tabs = settings.restored_session_tabs();
        assert_eq!(tabs[0].title, "example.test");
        assert_eq!(tabs[1].title, "other.test");
        assert_eq!(tabs[2].title, "New tab");
    }
    #[test]
    fn notification_settings_are_backward_compatible_and_serialize_only_explicit_regular_choices() {
        let old: Settings =
            serde_json::from_str(r#"{"session":["https://example.test"]}"#).unwrap();
        assert!(old.notification_permissions.is_empty());
        let mut settings = old;
        settings
            .notification_permissions
            .insert("https://example.test".into(), "allow".into());
        let encoded = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            restored
                .notification_permissions
                .get("https://example.test")
                .map(String::as_str),
            Some("allow")
        );
        assert_eq!(restored.session, vec!["https://example.test"]);
    }
    fn settings_fixture(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "yas-settings-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    fn remove_settings_fixture(root: PathBuf) {
        let resolved = fs::canonicalize(&root).unwrap();
        assert!(resolved.starts_with(fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("yas-settings-"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn settings_defaults_only_for_a_missing_file_and_preserves_legacy_sessions() {
        let missing_root = settings_fixture("missing");
        let missing = read_settings(&missing_root).unwrap();
        assert!(missing.bookmarks.is_empty());
        assert!(!missing.background_private_downloads);
        remove_settings_fixture(missing_root);
        let root = settings_fixture("valid");
        fs::write(root.join("settings.json"), br#"{"session":["goodbro://newtab"],"bookmarks":[{"title":"Saved site","url":"https://saved.example/"}]}"#).unwrap();
        let loaded = read_settings(&root).unwrap();
        assert_eq!(loaded.session, ["yas://newtab"]);
        assert_eq!(loaded.bookmarks[0].url, "https://saved.example/");
        assert!(!loaded.background_private_downloads);
        remove_settings_fixture(root);
    }
    #[test]
    fn corrupt_settings_are_rejected_without_replacing_the_original_file() {
        let root = settings_fixture("corrupt");
        let original = br#"{"bookmarks":[{"title":"Saved site"}"#;
        fs::write(root.join("settings.json"), original).unwrap();
        let error = read_settings(&root)
            .err()
            .expect("Corrupt settings must stop loading");
        assert!(error.contains("existing file was preserved"));
        assert_eq!(fs::read(root.join("settings.json")).unwrap(), original);
        fs::remove_file(root.join("settings.json")).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn locked_settings_are_not_treated_as_an_empty_profile() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = settings_fixture("locked");
        let path = root.join("settings.json");
        let original = br#"{"bookmarks":[{"url":"https://saved.example/"}]}"#;
        fs::write(&path, original).unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(read_settings(&root).is_err());
        drop(held);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(read_settings(&root).unwrap().bookmarks.len(), 1);
        remove_settings_fixture(root);
    }
    #[test]
    fn origins_are_exact_and_https_only() {
        assert_eq!(
            origin("https://example.com:8443/a"),
            Some("https://example.com:8443".into())
        );
        assert_ne!(
            origin("https://example.com"),
            origin("https://example.com.evil.test")
        );
        assert_eq!(origin("http://example.com"), None);
        assert_eq!(origin("file:///a"), None);
    }
    #[test]
    fn address_resolution() {
        assert_eq!(destination("example.com"), "https://example.com/");
        assert_eq!(destination("localhost:9000/a"), "http://localhost:9000/a");
        assert!(destination("rust browser").contains("q=rust+browser"));
        assert!(destination("javascript:alert(1)").starts_with("https://www.bing.com/"));
        assert!(destination("javascript:document.title=\"injected\"")
            .starts_with("https://www.bing.com/"));
        assert_eq!(
            destination("example.com:8443/a"),
            "https://example.com:8443/a"
        );
    }
    #[test]
    fn selected_search_provider_encodes_queries_without_changing_web_addresses() {
        for (engine, host) in [
            ("bing", "www.bing.com"),
            ("google", "www.google.com"),
            ("duckduckgo", "duckduckgo.com"),
            ("brave", "search.brave.com"),
        ] {
            let address = destination_with_engine("weather دبي & tomorrow", engine);
            let url = Url::parse(&address).unwrap();
            assert_eq!(url.host_str(), Some(host));
            assert_eq!(
                url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
                "weather دبي & tomorrow"
            );
            assert_eq!(
                destination_with_engine("https://example.test/path?q=one#two", engine),
                "https://example.test/path?q=one#two"
            );
            assert_eq!(
                destination_with_engine("localhost:9000/a", engine),
                "http://localhost:9000/a"
            );
        }
        assert!(destination_with_engine("javascript:alert(1)", "google")
            .starts_with("https://www.google.com/search?"));
        assert!(destination_with_engine("a question", "invalid")
            .starts_with("https://www.bing.com/search?"));
    }
    #[test]
    fn old_preferences_get_safe_defaults_and_user_choices_roundtrip() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.appearance, "system");
        assert_eq!(settings.search_engine, "bing");
        assert_eq!(settings.local_ai_model, "qwen3:0.6b");
        settings.appearance = "dark".into();
        settings.search_engine = "duckduckgo".into();
        let decoded: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(decoded.appearance, "dark");
        assert_eq!(decoded.search_engine, "duckduckgo");
    }
    #[test]
    fn new_tab_urls_survive_the_brand_upgrade() {
        assert_eq!(destination(crate::identity::LEGACY_NEW_TAB), "yas://newtab");
        assert_eq!(destination("yas://newtab"), "yas://newtab");
    }
    #[test]
    fn dpapi_roundtrip() {
        let data = b"test-credential";
        let encrypted = protect(data, false).unwrap();
        assert_ne!(encrypted, data);
        assert_eq!(protect(&encrypted, true).unwrap(), data);
    }
    #[test]
    fn edge_csv_import_is_atomic_and_deduplicates() {
        let root = settings_fixture("csv");
        let csv = root.join("test.csv");
        fs::write(&csv,"name,url,username,password\nDemo,https://example.com/login,alice,one\nDemo,https://example.com/other,alice,two\nHTTP,http://example.org,bob,ignored\n").unwrap();
        let mut vault = Vault::default();
        assert_eq!(import_logins(csv.clone(), &mut vault).unwrap(), 3);
        assert_eq!(vault.logins.len(), 2);
        assert_eq!(
            vault
                .logins
                .iter()
                .find(|l| l.username == "alice")
                .unwrap()
                .password,
            "two"
        );
        write_vault(&root, &vault).unwrap();
        write_vault(&root, &vault).unwrap();
        assert_eq!(read_vault(&root).unwrap().logins[0].username, "alice");
        fs::write(&csv, "url,username\nhttps://bad.example,bob\n").unwrap();
        assert!(import_logins(csv.clone(), &mut vault).is_err());
        assert_eq!(vault.logins.len(), 2);
        remove_settings_fixture(root);
    }
    #[test]
    fn exported_http_android_credentials_are_stored_but_not_https_fill_scopes() {
        let root = settings_fixture("scopes");
        let csv = root.join("scopes.csv");
        fs::write(&csv,"name,url,username,password\nWebsite,https://same.example/login,alice,https-value\nLegacy HTTP,http://same.example/login,alice,http-value\nFirst Android,android://hash@com.example.one/,alice,android-old\nUpdated Android,android://hash@com.example.one/,alice,android-new\nSecond Android,android://hash2@com.example.two/,alice,android-two\nBad JS,javascript:alert(1),alice,rejected\nBad file,file:///secret,alice,rejected\nBad data,data:secret,alice,rejected\nBad ftp,ftp://example.com/,alice,rejected\n").unwrap();
        let mut vault = Vault::default();
        assert_eq!(import_logins(csv.clone(), &mut vault).unwrap(), 5);
        assert_eq!(vault.logins.len(), 4);
        assert_eq!(
            login_scope("http://same.example/login").as_deref(),
            Some("http://same.example")
        );
        let http = vault
            .logins
            .iter()
            .find(|l| l.origin == "http://same.example")
            .unwrap();
        assert_eq!(http.label.as_deref(), Some("Legacy HTTP"));
        assert!(origin(&http.origin).is_none());
        let android = vault
            .logins
            .iter()
            .find(|l| l.origin == "android://hash@com.example.one/")
            .unwrap();
        assert_eq!(android.password, "android-new");
        assert_eq!(android.label.as_deref(), Some("Updated Android"));
        assert!(origin(&android.origin).is_none());
        assert_eq!(
            vault
                .logins
                .iter()
                .filter(|l| l.origin.starts_with("android:"))
                .count(),
            2
        );
        assert!(vault
            .logins
            .iter()
            .any(|l| l.origin == "https://same.example"));
        for input in [
            "javascript:alert(1)",
            "file:///secret",
            "data:secret",
            "ftp://example.com/",
        ] {
            assert!(login_scope(input).is_none());
        }
        write_vault(&root, &vault).unwrap();
        let read = read_vault(&root).unwrap();
        assert_eq!(read.logins.len(), 4);
        assert!(!String::from_utf8_lossy(
            &fs::read(root.join(crate::database::FILE_NAME)).unwrap()
        )
        .contains("android-new"));
        remove_settings_fixture(root);
    }
}
