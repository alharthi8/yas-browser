use serde::{Deserialize, Serialize};
use std::{
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
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub bookmarks: Vec<Entry>,
    pub history: Vec<Entry>,
    pub session: Vec<String>,
    pub exceptions: Vec<String>,
    pub custom_filters: String,
    pub imported_tabs: Vec<Entry>,
    pub reading_list: Vec<Entry>,
    pub background_private_downloads: bool,
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
    let mut u = Url::parse("https://www.bing.com/search").unwrap();
    u.query_pairs_mut().append_pair("q", s);
    u.into()
}
pub fn read_settings(root: &Path) -> Result<Settings, String> {
    let bytes = match fs::read(root.join("settings.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(error) => {
            return Err(format!(
                "Could not read settings.json: {error}. The existing file was preserved."
            ))
        }
    };
    let mut settings: Settings = serde_json::from_slice(&bytes).map_err(|error| {
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
    atomic_write(
        &root.join("settings.json"),
        &serde_json::to_vec_pretty(data).map_err(|e| e.to_string())?,
    )
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
    let p = root.join("vault.dpapi");
    if !p.exists() {
        return Ok(Vault::default());
    }
    let plain = protect(&fs::read(p).map_err(|e| e.to_string())?, true)?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}
pub fn write_vault(root: &Path, data: &Vault) -> Result<(), String> {
    let mut plain = serde_json::to_vec(data).map_err(|e| e.to_string())?;
    let encrypted = protect(&plain, false);
    plain.fill(0);
    atomic_write(&root.join("vault.dpapi"), &encrypted?)
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
    #[test]
    fn settings_defaults_only_for_a_missing_file_and_preserves_legacy_sessions() {
        let root = settings_fixture("valid");
        let missing = read_settings(&root).unwrap();
        assert!(missing.bookmarks.is_empty());
        assert!(!missing.background_private_downloads);
        fs::write(root.join("settings.json"), br#"{"session":["goodbro://newtab"],"bookmarks":[{"title":"Saved site","url":"https://saved.example/"}]}"#).unwrap();
        let loaded = read_settings(&root).unwrap();
        assert_eq!(loaded.session, ["yas://newtab"]);
        assert_eq!(loaded.bookmarks[0].url, "https://saved.example/");
        assert!(!loaded.background_private_downloads);
        fs::remove_file(root.join("settings.json")).unwrap();
        fs::remove_dir(root).unwrap();
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
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
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
        let root = std::env::temp_dir().join(format!("yas-csv-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
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
        fs::remove_file(csv).unwrap();
        fs::remove_file(root.join("vault.dpapi")).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn exported_http_android_credentials_are_stored_but_not_https_fill_scopes() {
        let root = std::env::temp_dir().join(format!("yas-scopes-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
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
        assert!(
            !String::from_utf8_lossy(&fs::read(root.join("vault.dpapi")).unwrap())
                .contains("android-new")
        );
        fs::remove_file(csv).unwrap();
        fs::remove_file(root.join("vault.dpapi")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
