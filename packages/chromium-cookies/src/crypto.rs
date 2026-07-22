//! Turn a `Safe Storage` password into an AES key and decrypt a cookie blob.
//!
//! macOS Chromium uses `AES-128-CBC` with a key of
//! `PBKDF2-HMAC-SHA1(password, "saltysalt", 1003, 16)` and a fixed 16-space IV.
//! Encrypted values carry a 3-byte `v10`/`v11` version tag. Recent Chromium also
//! prepends a 32-byte `SHA-256` domain hash to the plaintext, which
//! [`decode_value`] strips when present.

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Result, anyhow, bail};

const SALT: &[u8] = b"saltysalt";
const ITERATIONS: u32 = 1003;
const KEY_LEN: usize = 16;
const IV: [u8; 16] = [b' '; 16];
const HASH_PREFIX_LEN: usize = 32;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Derive the 16-byte AES key from a `Safe Storage` password.
#[must_use]
pub fn derive_key(password: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, SALT, ITERATIONS, &mut key);
    key
}

/// Decrypt one `encrypted_value` blob to raw plaintext (hash prefix intact).
///
/// # Errors
/// Fails when the blob lacks a known version tag, has a non-block-aligned body,
/// or the `AES`/`PKCS7` decrypt does not validate.
pub fn decrypt(blob: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let body = match blob.get(..3) {
        Some(b"v10" | b"v11") => &blob[3..],
        _ => bail!("unrecognized cookie encryption (want a v10/v11 prefix)"),
    };
    if body.is_empty() || body.len() % 16 != 0 {
        let len = body.len();
        bail!("ciphertext length {len} is not a block multiple");
    }

    let mut buf = body.to_vec();
    let pt = Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|e| anyhow!("AES/PKCS7 decrypt failed: {e}"))?;
    Ok(pt.to_vec())
}

/// Decode plaintext to a cookie string, stripping the 32-byte domain-hash prefix
/// newer Chromium prepends. Printable UTF-8 as-is wins; otherwise the tail after
/// the prefix is tried.
#[must_use]
pub fn decode_value(plaintext: &[u8]) -> String {
    fn printable(bytes: &[u8]) -> Option<String> {
        std::str::from_utf8(bytes)
            .ok()
            .filter(|s| s.chars().all(|c| !c.is_control() || c == '\t'))
            .map(str::to_owned)
    }

    printable(plaintext)
        .or_else(|| plaintext.get(HASH_PREFIX_LEN..).and_then(printable))
        .unwrap_or_else(|| {
            let len = plaintext.len();
            format!("<{len} binary bytes>")
        })
}
