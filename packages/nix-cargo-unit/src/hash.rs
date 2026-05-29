use sha2::{Digest as _, Sha256};

/// Lowercase hex encoding of `bytes` (two characters per byte).
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

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
    hex(&digest[..8])
}

#[cfg(kani)]
mod verification {
    use super::*;

    const fn nibble_to_hex(nibble: u8) -> u8 {
        if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + (nibble - 10)
        }
    }

    #[kani::proof]
    #[kani::unwind(9)]
    fn short_digest_encodes_every_byte_as_lowercase_hex() {
        let digest: [u8; 32] = kani::any();
        let encoded = short_digest(&digest);
        let encoded_bytes = encoded.as_bytes();

        assert_eq!(encoded_bytes.len(), 8 * 2);
        for index in 0..8 {
            let byte = digest[index];
            let high = encoded_bytes[index * 2];
            let low = encoded_bytes[index * 2 + 1];

            assert_eq!(high, nibble_to_hex(byte >> 4));
            assert_eq!(low, nibble_to_hex(byte & 0x0f));
            assert!((b'0' <= high && high <= b'9') || (b'a' <= high && high <= b'f'));
            assert!((b'0' <= low && low <= b'9') || (b'a' <= low && low <= b'f'));
        }
    }
}
