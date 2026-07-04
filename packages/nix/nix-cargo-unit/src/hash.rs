use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};

/// First 8 bytes of the SHA-256 of `value` as 16 lowercase hex characters.
/// This is the short identity stamp shared by unit names, source-store names,
/// and rustc `-C metadata`, so every site that needs a stable, collision
/// resistant tag derives it the same way.
pub fn short(value: &str) -> String {
    short_digest(&Sha256::digest(value.as_bytes()))
}

/// 16 lowercase hex characters from the leading 8 bytes of a finished digest.
/// Use this when the digest is built incrementally rather than from one string.
pub fn short_digest(digest: &[u8]) -> String {
    let mut out = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
