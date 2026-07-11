//! Content-addressed blobs for shared-audio.
//!
//! The score names an instrument module by content hash; the bytes travel
//! out of band (peer gossip) and land here. One hash type and one store so
//! the score, wasm host, and network layers agree without depending on each
//! other.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// SHA-256 content address of a blob (an instrument module's bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobHash([u8; 32]);

impl BlobHash {
    /// Hash `bytes` into its content address.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The raw 32-byte digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Rebuild a hash from its raw 32-byte digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Parse the lowercase-hex form produced by [`fmt::Display`].
    ///
    /// # Errors
    ///
    /// Returns an error when `text` is not exactly 64 hex characters.
    pub fn parse_hex(text: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0_u8; 32];
        hex::decode_to_slice(text, &mut bytes)?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobHash({self})")
    }
}

/// Directory-backed blob store: one file per blob, named by its hex hash.
///
/// Writes go through a temp file plus rename so a crash never leaves a
/// half-written blob under its final name, and reads re-hash the bytes so a
/// corrupt file surfaces as an error instead of as wrong audio.
#[derive(Debug, Clone)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    /// Open (creating if needed) a store rooted at `dir`.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be created.
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The directory backing this store.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path_of(&self, hash: &BlobHash) -> PathBuf {
        self.dir.join(hash.to_string())
    }

    /// Store `bytes`, returning their content address. Idempotent, and the
    /// atomic rewrite repairs a corrupt or partial file of the same name.
    ///
    /// # Errors
    ///
    /// Returns an error when the blob file cannot be written.
    pub fn put(&self, bytes: &[u8]) -> io::Result<BlobHash> {
        let hash = BlobHash::of(bytes);
        let path = self.path_of(&hash);
        let tmp = self.dir.join(format!("{hash}.tmp"));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)?;
        Ok(hash)
    }

    /// Fetch a blob's bytes, verifying them against the requested hash.
    ///
    /// Returns `Ok(None)` when the blob is absent.
    ///
    /// # Errors
    ///
    /// Returns an error on read failure or when the stored bytes no longer
    /// match their name (on-disk corruption).
    pub fn get(&self, hash: &BlobHash) -> io::Result<Option<Vec<u8>>> {
        let path = self.path_of(hash);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if BlobHash::of(&bytes) == *hash {
            Ok(Some(bytes))
        } else {
            Err(io::Error::other(format!("blob {hash} is corrupt on disk")))
        }
    }

    /// Whether the store currently holds a *valid* blob for `hash`. A
    /// corrupt or partial file does not count, so callers keep fetching (or
    /// re-`put`ting) until the verified bytes are on disk.
    #[must_use]
    pub fn contains(&self, hash: &BlobHash) -> bool {
        matches!(self.get(hash), Ok(Some(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_idempotent_put() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("open");
        let hash = store.put(b"instrument bytes").expect("put");
        assert_eq!(store.put(b"instrument bytes").expect("re-put"), hash);
        assert!(store.contains(&hash));
        assert_eq!(
            store.get(&hash).expect("get"),
            Some(b"instrument bytes".to_vec())
        );
    }

    #[test]
    fn absent_blob_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("open");
        let hash = BlobHash::of(b"never stored");
        assert_eq!(store.get(&hash).expect("get"), None);
        assert!(!store.contains(&hash));
    }

    #[test]
    fn corrupt_blob_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).expect("open");
        let hash = store.put(b"good bytes").expect("put");
        std::fs::write(dir.path().join(hash.to_string()), b"tampered").expect("tamper");
        assert!(store.get(&hash).is_err());
        // A corrupt file is not "present", and a re-put repairs it.
        assert!(!store.contains(&hash));
        assert_eq!(store.put(b"good bytes").expect("re-put"), hash);
        assert!(store.contains(&hash));
        assert_eq!(store.get(&hash).expect("get"), Some(b"good bytes".to_vec()));
    }

    #[test]
    fn hex_roundtrip() {
        let hash = BlobHash::of(b"abc");
        let parsed = BlobHash::parse_hex(&hash.to_string()).expect("parse");
        assert_eq!(parsed, hash);
        assert!(BlobHash::parse_hex("zz").is_err());
    }
}
