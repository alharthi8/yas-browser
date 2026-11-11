use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const FEED_URL: &str =
    "https://github.com/alharthi8/yas-browser/releases/latest/download/yas-windows-stable.json";
pub const MAX_FEED: u64 = 32 * 1024;
pub const MAX_INSTALLER: u64 = 150 * 1024 * 1024;
pub const PUBLIC_KEY: &[u8; 32] = include_bytes!("../release.pub");

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub schema: u32,
    pub product: String,
    pub platform: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub notes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    payload: String,
    signature: String,
}

pub fn version(value: &str) -> Result<[u16; 3], String> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3 {
        return Err("The update version is invalid.".into());
    }
    let mut parsed = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty()
            || (part.len() > 1 && part.starts_with('0'))
            || !part.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("The update version is invalid.".into());
        }
        parsed[i] = part.parse().map_err(|_| "The update version is invalid.")?;
    }
    Ok(parsed)
}

pub fn verify(bytes: &[u8], key: &[u8; 32], now: u64) -> Result<Release, String> {
    if bytes.len() as u64 > MAX_FEED {
        return Err("The update feed is too large.".into());
    }
    let envelope: Envelope =
        serde_json::from_slice(bytes).map_err(|_| "The update feed is invalid.")?;
    let payload = STANDARD
        .decode(envelope.payload)
        .map_err(|_| "The update payload is invalid.")?;
    let signature = STANDARD
        .decode(envelope.signature)
        .map_err(|_| "The update signature is invalid.")?;
    let signature =
        Signature::from_slice(&signature).map_err(|_| "The update signature is invalid.")?;
    VerifyingKey::from_bytes(key)
        .map_err(|_| "The update verification key is invalid.")?
        .verify_strict(&payload, &signature)
        .map_err(|_| "Update signature verification failed. Nothing was installed.")?;
    let release: Release =
        serde_json::from_slice(&payload).map_err(|_| "The signed update data is invalid.")?;
    validate(&release, now)?;
    Ok(release)
}

fn validate(release: &Release, now: u64) -> Result<(), String> {
    version(&release.version)?;
    if release.schema != 1
        || release.product != "yas-browser"
        || release.platform != "windows-x86_64"
        || release.bytes == 0
        || release.bytes > MAX_INSTALLER
        || release.sha256.len() != 64
        || !release.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        || release.notes.len() > 4096
        || release.issued_at > now.saturating_add(300)
        || release.expires_at <= now
        || release.expires_at <= release.issued_at
        || release.expires_at - release.issued_at > 90 * 86400
    {
        return Err("The signed update metadata is unsupported or expired.".into());
    }
    let expected = format!(
        "https://github.com/alharthi8/yas-browser/releases/download/v{0}/Yas-{0}-Setup-x64.exe",
        release.version
    );
    if release.url != expected {
        return Err("The update download address is not trusted.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    const NOW: u64 = 1_800_000_000;
    fn release() -> Release {
        Release {
            schema: 1,
            product: "yas-browser".into(),
            platform: "windows-x86_64".into(),
            version: "0.4.10".into(),
            url: "https://github.com/alharthi8/yas-browser/releases/download/v0.4.10/Yas-0.4.10-Setup-x64.exe".into(),
            sha256: "ab".repeat(32),
            bytes: 25_000_000,
            issued_at: NOW - 60,
            expires_at: NOW + 86400,
            notes: "A tested update".into(),
        }
    }
    fn signed(release: &Release) -> (Vec<u8>, [u8; 32]) {
        let key = SigningKey::from_bytes(&[91; 32]);
        let payload = serde_json::to_vec(release).unwrap();
        let signature = key.sign(&payload);
        (
            serde_json::to_vec(&serde_json::json!({"payload":STANDARD.encode(payload),"signature":STANDARD.encode(signature.to_bytes())})).unwrap(),
            key.verifying_key().to_bytes(),
        )
    }
    #[test]
    fn versions_are_numeric_canonical_and_bounded() {
        assert!(version("0.4.10").unwrap() > version("0.4.9").unwrap());
        for invalid in [
            "0.04.9",
            "0.4",
            "0.4.9.0",
            "0.4.-1",
            "0.4.9-beta",
            "0.4.65536",
        ] {
            assert!(version(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn valid_signature_is_required_before_release_data_is_trusted() {
        let (mut data, key) = signed(&release());
        assert_eq!(verify(&data, &key, NOW).unwrap().version, "0.4.10");
        assert!(verify(&data, &[8; 32], NOW).is_err());
        let index = data.iter().position(|b| *b == b'A').unwrap();
        data[index] = b'B';
        assert!(verify(&data, &key, NOW).is_err());
        assert!(verify(&vec![b' '; MAX_FEED as usize + 1], &key, NOW).is_err());
    }
    #[test]
    fn even_signed_untrusted_locations_and_malformed_metadata_are_rejected() {
        let baseline = release();
        let mutations: Vec<fn(&mut Release)> = vec![
            |r| r.url = r.url.replace("https:", "http:"),
            |r| r.url = r.url.replace("github.com", "github.com.evil.test"),
            |r| r.url.push_str("?redirect=evil"),
            |r| r.url = r.url.replace("https://", "https://user:password@"),
            |r| r.version = "0.4.9-beta".into(),
            |r| r.platform = "linux-x86_64".into(),
            |r| r.product = "another-product".into(),
            |r| r.bytes = MAX_INSTALLER + 1,
            |r| r.bytes = 0,
            |r| r.sha256 = "x".repeat(64),
            |r| r.expires_at = NOW,
            |r| r.issued_at = NOW + 301,
            |r| r.expires_at = NOW + 91 * 86400,
            |r| r.notes = "x".repeat(4097),
        ];
        for mutate in mutations {
            let mut changed = baseline.clone();
            mutate(&mut changed);
            let (data, key) = signed(&changed);
            assert!(verify(&data, &key, NOW).is_err());
        }
    }
}
