//! Content addressing.
//!
//! A file's identity is the hash of its bytes, never its path. Byte-identical
//! files across twenty git worktrees (or branches, or repos) hash to one
//! value, so the expensive embedding upload happens once and every other
//! checkout reuses the existing entry instead of re-indexing.

use std::fmt::{self, Write as _};

use sha2::{Digest, Sha256};

/// Stable content identifier for a file, formatted as `sha256:<hex>`.
///
/// Used directly as the Mixedbread `external_id`, which is what makes the
/// store deduplicate: two uploads with the same content produce the same id,
/// so the second is a no-op the sync skips entirely.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ContentHash(String);

impl ContentHash {
    /// Compute the content hash of a byte buffer.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut hex = String::with_capacity("sha256:".len() + digest.len() * 2);
        hex.push_str("sha256:");
        for byte in digest {
            // Writing a byte to a String is infallible; ignore the Result
            // rather than introduce an unreachable panic path.
            let _ = write!(hex, "{byte:02x}");
        }
        Self(hex)
    }

    /// The hash as a string slice, e.g. `sha256:abcd…`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The hex digest without the `sha256:` algorithm prefix. Useful as a
    /// filesystem-safe cache key component.
    #[must_use]
    pub fn hex(&self) -> &str {
        self.0.strip_prefix("sha256:").unwrap_or(&self.0)
    }

    /// Wrap a hash string read back from the manifest database.
    #[must_use]
    pub const fn from_raw(raw: String) -> Self {
        Self(raw)
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::ContentHash;

    #[test]
    fn identical_bytes_hash_equal() {
        let a = ContentHash::of_bytes(b"fn main() {}");
        let b = ContentHash::of_bytes(b"fn main() {}");
        assert_eq!(a, b, "same content must produce the same id (dedup key)");
    }

    #[test]
    fn different_bytes_hash_differently() {
        let a = ContentHash::of_bytes(b"one");
        let b = ContentHash::of_bytes(b"two");
        assert_ne!(a, b);
    }

    #[test]
    fn format_is_prefixed_lowercase_hex() {
        let h = ContentHash::of_bytes(b"");
        // sha256 of the empty input is a known constant.
        assert_eq!(
            h.as_str(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(h.hex(), &h.as_str()["sha256:".len()..]);
    }
}
