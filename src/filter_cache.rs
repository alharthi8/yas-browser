//! Optional immutable cache of the unchanged bundled filters, never a local DAT file.
//! Any unverified input/build/hash/decode falls back to the ordinary text engine.
use adblock::Engine;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[path = "../filters/cache_sha256.rs"]
mod crypto;

const PAYLOAD: &[u8] = include_bytes!("../filters/bundled-engine.dat");
const METADATA: &str = include_str!("../filters/bundled-engine.json");
const BUILD_ATTESTATION: Option<&str> = option_env!("YAS_FILTER_CACHE_BUILD_ATTESTATION");
pub(super) const POLICY: &str = "Yas-bundled-filter-cache-v1;Engine::new_with_list_text;FilterSet::new(false);ParseOptions::default;unchanged-effective-input-only;no-added-resources-or-tags";
const MAX_BYTES: usize = 64 * 1024 * 1024;
const FILES: [(&str, &[u8]); 7] = [
    ("Cargo.toml", include_bytes!("../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../Cargo.lock")),
    ("src/blocker.rs", include_bytes!("blocker.rs")),
    ("src/filter_cache.rs", include_bytes!("filter_cache.rs")),
    (
        "filters/cache_sha256.rs",
        include_bytes!("../filters/cache_sha256.rs"),
    ),
    (
        "filters/generate_bundled_cache.rs",
        include_bytes!("../filters/generate_bundled_cache.rs"),
    ),
    ("build.ps1", include_bytes!("../build.ps1")),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fingerprint {
    bytes: usize,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema: u32,
    constructor_policy: String,
    input: Fingerprint,
    payload: Fingerprint,
    sources: BTreeMap<String, String>,
    build: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum Miss {
    MissingBuild,
    Metadata,
    Build,
    Size,
    Input,
    Source,
    Payload,
    Crypto,
    Decode,
}

fn matches(
    bytes: &[u8],
    expected: &str,
    hash: fn(&[u8]) -> Option<[u8; 32]>,
) -> Result<bool, Miss> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Miss::Metadata);
    }
    let digest = hash(bytes).ok_or(Miss::Crypto)?;
    Ok(crypto::hex(digest).eq_ignore_ascii_case(expected))
}

fn checked(
    input: &str,
    payload: &[u8],
    metadata: &str,
    attestation: Option<&str>,
    files: &[(&str, &[u8])],
    hash: fn(&[u8]) -> Option<[u8; 32]>,
) -> Result<Engine, Miss> {
    let stamp = attestation.ok_or(Miss::MissingBuild)?;
    if metadata.len() > 64 * 1024 || input.len() > MAX_BYTES || payload.len() > MAX_BYTES {
        return Err(Miss::Size);
    }
    let metadata: Metadata = serde_json::from_str(metadata).map_err(|_| Miss::Metadata)?;
    if metadata.schema != 1 || metadata.constructor_policy != POLICY {
        return Err(Miss::Metadata);
    }
    let stamp: Value = serde_json::from_str(stamp).map_err(|_| Miss::Build)?;
    if stamp != metadata.build || stamp["schema"] != 1 || stamp["profile"] != "release" {
        return Err(Miss::Build);
    }
    if input.len() != metadata.input.bytes
        || !matches(input.as_bytes(), &metadata.input.sha256, hash)?
    {
        return Err(Miss::Input);
    }
    if files.len() != metadata.sources.len() {
        return Err(Miss::Source);
    }
    for (path, bytes) in files {
        let expected = metadata.sources.get(*path).ok_or(Miss::Source)?;
        if !matches(bytes, expected, hash)? {
            return Err(Miss::Source);
        }
    }
    // adblock's same-version path skips FlatBuffer verification. Validate the
    // complete embedded bytes against immutable SHA-256 metadata FIRST.
    if payload.len() != metadata.payload.bytes || !matches(payload, &metadata.payload.sha256, hash)?
    {
        return Err(Miss::Payload);
    }
    let mut engine = Engine::default();
    #[cfg(test)]
    DECODE_CALLS.with(|calls| calls.set(calls.get() + 1));
    engine.deserialize(payload).map_err(|_| Miss::Decode)?;
    Ok(engine)
}

pub(super) fn build(input: String) -> (Engine, bool) {
    let cached = checked(
        &input,
        PAYLOAD,
        METADATA,
        BUILD_ATTESTATION,
        &FILES,
        crypto::sha256,
    );
    finish(input, cached)
}

fn finish(input: String, cached: Result<Engine, Miss>) -> (Engine, bool) {
    match cached {
        Ok(engine) => (engine, true),
        Err(_) => (Engine::new_with_list_text(input), false),
    }
}

#[cfg(test)]
thread_local! {
    static DECODE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use adblock::request::Request;

    fn input() -> String {
        let mut input = String::from(super::super::FALLBACK);
        input.push_str(include_str!("../filters/easylist.txt"));
        input.push('\n');
        input.push_str(include_str!("../filters/easyprivacy.txt"));
        input.push('\n');
        input
    }

    fn stamp() -> String {
        let metadata: Value = serde_json::from_str(METADATA).unwrap();
        serde_json::to_string(&metadata["build"]).unwrap()
    }

    fn with_stamp(input: &str, stamp: Option<&str>) -> Result<Engine, Miss> {
        checked(input, PAYLOAD, METADATA, stamp, &FILES, crypto::sha256)
    }

    #[test]
    fn native_sha256_known_vectors() {
        assert_eq!(
            crypto::hex(crypto::sha256(b"").unwrap()),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
        assert_eq!(
            crypto::hex(crypto::sha256(b"abc").unwrap()),
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        );
    }

    #[test]
    fn bundled_network_and_cosmetic_behavior_matches_text_engine() {
        let input = input();
        let cached = with_stamp(&input, Some(&stamp())).unwrap();
        let plain = Engine::new_with_list_text(input);
        for (url, source, kind, method) in [
            (
                "https://doubleclick.net/ad.js",
                "https://news.example/",
                "script",
                "GET",
            ),
            (
                "https://google-analytics.com/collect",
                "https://example.com/",
                "xmlhttprequest",
                "POST",
            ),
            (
                "https://connect.facebook.net/en_US/fbevents.js",
                "https://example.com/",
                "script",
                "GET",
            ),
            (
                "https://example.com/content.js",
                "https://example.com/",
                "script",
                "GET",
            ),
            (
                "https://github.githubassets.com/assets/main.js",
                "https://github.com/rust-lang/rust",
                "script",
                "GET",
            ),
            (
                "https://developer.mozilla.org/static/css/main.css",
                "https://developer.mozilla.org/",
                "stylesheet",
                "GET",
            ),
            (
                "http://127.0.0.1:9397/a.js",
                "http://localhost:9397/",
                "script",
                "GET",
            ),
            (
                "https://xn--bcher-kva.example/a.png",
                "https://bücher.example/",
                "image",
                "GET",
            ),
            (
                "wss://example.com/socket",
                "https://example.com/",
                "websocket",
                "GET",
            ),
        ] {
            let request = Request::new(url, source, kind, method).unwrap();
            assert_eq!(
                serde_json::to_value(plain.check_network_request(&request)).unwrap(),
                serde_json::to_value(cached.check_network_request(&request)).unwrap()
            );
        }
        for url in [
            "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "https://github.com/rust-lang/rust",
            "https://developer.mozilla.org/en-US/docs/Web/JavaScript",
            "https://www.bbc.com/news",
            "https://appana.ae/business",
            "https://example.com/",
        ] {
            let a = plain.url_cosmetic_resources(url);
            let b = cached.url_cosmetic_resources(url);
            assert_eq!(a, b);
            let mut ga = plain.hidden_class_id_selectors(
                ["ad", "ads", "advertisement", "normal"],
                ["ad", "content", ""],
                &a.exceptions,
            );
            let mut gb = cached.hidden_class_id_selectors(
                ["ad", "ads", "advertisement", "normal"],
                ["ad", "content", ""],
                &b.exceptions,
            );
            ga.sort();
            gb.sort();
            assert_eq!(ga, gb);
        }
    }

    #[test]
    fn missing_build_and_changed_version_features_compiler_fail_closed() {
        let input = input();
        assert!(matches!(with_stamp(&input, None), Err(Miss::MissingBuild)));
        for field in ["engine_version", "engine_features", "compiler", "profile"] {
            let mut value: Value = serde_json::from_str(&stamp()).unwrap();
            value[field] = serde_json::json!("changed-build-configuration");
            assert!(matches!(
                with_stamp(&input, Some(&value.to_string())),
                Err(Miss::Build)
            ));
        }
    }

    #[test]
    fn changed_manifest_lock_or_constructor_sources_do_not_reuse_cache() {
        let input = input();
        let stamp = stamp();
        for index in 0..FILES.len() {
            let mut files = FILES;
            files[index].1 = b"changed source or engine configuration";
            assert!(matches!(
                checked(
                    &input,
                    PAYLOAD,
                    METADATA,
                    Some(&stamp),
                    &files,
                    crypto::sha256
                ),
                Err(Miss::Source)
            ));
        }
        let mut metadata: Value = serde_json::from_str(METADATA).unwrap();
        metadata["constructor_policy"] = "changed-constructor".into();
        assert!(matches!(
            checked(
                &input,
                PAYLOAD,
                &metadata.to_string(),
                Some(&stamp),
                &FILES,
                crypto::sha256
            ),
            Err(Miss::Metadata)
        ));
    }

    #[test]
    fn corrupted_payload_and_crypto_failure_never_reach_unchecked_decoder() {
        let input = input();
        let stamp = stamp();
        let before = DECODE_CALLS.with(std::cell::Cell::get);
        let mut corrupt = PAYLOAD.to_vec();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(matches!(
            checked(
                &input,
                &corrupt,
                METADATA,
                Some(&stamp),
                &FILES,
                crypto::sha256
            ),
            Err(Miss::Payload)
        ));
        assert!(matches!(
            checked(&input, PAYLOAD, METADATA, Some(&stamp), &FILES, |_| None),
            Err(Miss::Crypto)
        ));
        assert_eq!(before, DECODE_CALLS.with(std::cell::Cell::get));
    }

    #[test]
    fn updated_or_custom_text_remains_authoritative() {
        let mut input = input();
        input.push_str("||changed-filter-fixture.invalid^\n");
        let attempt = with_stamp(&input, Some(&stamp()));
        assert!(matches!(attempt, Err(Miss::Input)));
        let (engine, used) = finish(input, attempt);
        assert!(!used);
        let request = Request::new(
            "https://changed-filter-fixture.invalid/a.js",
            "https://example.com/",
            "script",
            "GET",
        )
        .unwrap();
        assert!(engine.check_network_request(&request).should_block());
    }

    #[test]
    fn invalid_metadata_and_truncated_payload_fall_back() {
        let input = input();
        let stamp = stamp();
        assert!(matches!(
            checked(&input, PAYLOAD, "{}", Some(&stamp), &FILES, crypto::sha256),
            Err(Miss::Metadata)
        ));
        assert!(matches!(
            checked(
                &input,
                &PAYLOAD[..12],
                METADATA,
                Some(&stamp),
                &FILES,
                crypto::sha256
            ),
            Err(Miss::Payload)
        ));
    }
}
