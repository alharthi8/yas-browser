//! Regenerate only through build.ps1 -GenerateBundledCache, using this package's
//! actual resolved dependencies/features. No profile or downloaded DAT is read.
use adblock::Engine;
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::Path};

#[path = "cache_sha256.rs"]
mod crypto;

const POLICY: &str = "Yas-bundled-filter-cache-v1;Engine::new_with_list_text;FilterSet::new(false);ParseOptions::default;unchanged-effective-input-only;no-added-resources-or-tags";
const SOURCES: [&str; 7] = [
    "Cargo.toml",
    "Cargo.lock",
    "src/blocker.rs",
    "src/filter_cache.rs",
    "filters/cache_sha256.rs",
    "filters/generate_bundled_cache.rs",
    "build.ps1",
];

fn hash(bytes: &[u8]) -> String {
    crypto::hex(crypto::sha256(bytes).expect("Windows SHA-256 failed; cache not generated"))
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(stamp) = option_env!("YAS_FILTER_CACHE_BUILD_ATTESTATION") else {
        eprintln!("Use the controlled build.ps1 -GenerateBundledCache path");
        std::process::exit(2);
    };
    let stamp: Value = serde_json::from_str(stamp).expect("Invalid controlled build stamp");
    assert_eq!(stamp["schema"], 1);
    assert_eq!(stamp["profile"], "release");

    let blocker = fs::read_to_string(root.join("src/blocker.rs")).unwrap();
    let declaration = blocker
        .lines()
        .find_map(|line| line.trim().strip_prefix("const FALLBACK: &str = "))
        .expect("The canonical FALLBACK string declaration was not found");
    let fallback: String = serde_json::from_str(declaration.trim_end_matches(';')).unwrap();
    let mut input = fallback;
    for name in ["easylist.txt", "easyprivacy.txt"] {
        input.push_str(&fs::read_to_string(root.join("filters").join(name)).unwrap());
        input.push('\n');
    }
    let engine = Engine::new_with_list_text(input.clone());
    let payload = engine.serialize();
    assert!(payload.len() <= 64 * 1024 * 1024);
    let mut sources = BTreeMap::new();
    for path in SOURCES {
        sources.insert(path, hash(&fs::read(root.join(path)).unwrap()));
    }
    let metadata = json!({
        "schema": 1, "constructor_policy": POLICY,
        "input": {"bytes": input.len(), "sha256": hash(input.as_bytes())},
        "payload": {"bytes": payload.len(), "sha256": hash(&payload)},
        "sources": sources, "build": stamp,
    });
    // An interrupted pair is harmless: the runtime payload SHA check fails and
    // the normal text engine is built before any website navigation.
    fs::write(root.join("filters/bundled-engine.dat"), &payload).unwrap();
    fs::write(
        root.join("filters/bundled-engine.json"),
        format!("{}\n", serde_json::to_string_pretty(&metadata).unwrap()),
    )
    .unwrap();
    println!(
        "{}",
        json!({"generated": true, "input": metadata["input"], "payload": metadata["payload"], "engine_version": metadata["build"]["engine_version"], "engine_features": metadata["build"]["engine_features"]})
    );
}
