//! Windows 10 CNG SHA-256; shared by the immutable-cache loader and generator.
//! https://learn.microsoft.com/windows/win32/api/bcrypt/nf-bcrypt-bcrypthash
use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

pub(super) fn sha256(bytes: &[u8]) -> Option<[u8; 32]> {
    // The generated Win32 wrapper converts this length to ULONG.
    if bytes.len() > u32::MAX as usize {
        return None;
    }
    let mut digest = [0; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, bytes, &mut digest) };
    (status.0 >= 0).then_some(digest)
}

pub(super) fn hex(digest: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 15) as usize] as char);
    }
    text
}
