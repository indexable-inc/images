//! Content-addressed storage: `ObjId -> bytes`.
//!
//! The memo table stores an [`ObjId`], never an output, so a row is small and
//! two rows agreeing on an answer share one copy of it.
//!
//! `put` and `get` both take `&self`. A content-addressed write is idempotent
//! and commutes with every other write, so exclusive access buys nothing, and
//! demanding `&mut self` here would force the eventual prolly-tree store to
//! hand out an exclusive handle it does not need. The in-memory implementation
//! pays for that with a mutex; the directory one does not need one at all.

use crate::error::{KernelError, Result};
use crate::id::ObjId;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// A store of immutable objects addressed by their content.
///
/// Implementations must satisfy: `get(put(b)) == Some(b)`, and `put` returns
/// [`ObjId::of(b)`] for every `b`. Everything else in the kernel is written
/// against those two sentences.
///
/// [`ObjId::of(b)`]: ObjId::of
pub trait Cas {
    /// Store bytes and return their address. Storing something already stored
    /// is a no-op that returns the same address.
    fn put(&self, bytes: &[u8]) -> Result<ObjId>;

    /// Load an object, or `None` if this store does not have it. Absence is
    /// not an error: a store is allowed to be a partial view, which is what
    /// makes a cache one of these.
    fn get(&self, id: ObjId) -> Result<Option<Vec<u8>>>;

    /// Whether the object is present. Overridable because a real store can
    /// usually answer this without moving the bytes.
    fn has(&self, id: ObjId) -> Result<bool> {
        Ok(self.get(id)?.is_some())
    }
}

/// In-memory store. For tests and for anything whose objects should not
/// outlive the process.
#[derive(Debug, Default)]
pub struct MemoryCas {
    objects: Mutex<BTreeMap<ObjId, Vec<u8>>>,
}

impl MemoryCas {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.objects().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects().is_empty()
    }

    /// Recovering from a poisoned lock is sound here because the only code
    /// inside the lock is a `BTreeMap` insert or lookup: there is no
    /// half-applied state a panicking thread could have left behind, and
    /// refusing to serve a cache because an unrelated thread panicked would
    /// turn one failure into a permanent one.
    fn objects(&self) -> std::sync::MutexGuard<'_, BTreeMap<ObjId, Vec<u8>>> {
        self.objects.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Cas for MemoryCas {
    fn put(&self, bytes: &[u8]) -> Result<ObjId> {
        let id = ObjId::of(bytes);
        self.objects().entry(id).or_insert_with(|| bytes.to_vec());
        Ok(id)
    }

    fn get(&self, id: ObjId) -> Result<Option<Vec<u8>>> {
        Ok(self.objects().get(&id).cloned())
    }

    fn has(&self, id: ObjId) -> Result<bool> {
        Ok(self.objects().contains_key(&id))
    }
}

/// Directory-backed store: one file per object, named by its address.
///
/// Flat rather than sharded. Sharding is a filesystem-performance decision
/// that depends on the object count, and this store is a stand-in until the
/// prolly-tree store lands, so adding a fan-out now would be guessing.
///
/// Writes go to a unique temporary file and are renamed into place, so a
/// reader never sees a partly written object and two concurrent writers of the
/// same object cannot interleave. Neither the file nor the directory is
/// fsynced: after a crash an object may be absent, which is a miss and
/// re-performs, but it can never be present and truncated.
#[derive(Clone, Debug)]
pub struct DirCas {
    root: PathBuf,
}

/// Distinguishes temporary files written by this process from each other; the
/// pid alone is not enough because one process writes many objects at once.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl DirCas {
    /// Open (creating if needed) a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|source| {
            KernelError::io(format!("creating store {}", root.display()), source)
        })?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn object_path(&self, id: ObjId) -> PathBuf {
        self.root.join(id.hash().to_hex())
    }
}

impl Cas for DirCas {
    fn put(&self, bytes: &[u8]) -> Result<ObjId> {
        let id = ObjId::of(bytes);
        let final_path = self.object_path(id);
        if final_path.exists() {
            return Ok(id);
        }

        let temp_path = self.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::File::create(&temp_path).map_err(|source| {
            KernelError::io(format!("creating {}", temp_path.display()), source)
        })?;
        let written = file
            .write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|source| KernelError::io(format!("writing {}", temp_path.display()), source));
        drop(file);
        if let Err(error) = written {
            // Best effort: the object is not addressable, so a leftover
            // temporary is the only trace, and failing to remove it must not
            // mask the write error that caused it.
            drop(fs::remove_file(&temp_path));
            return Err(error);
        }

        fs::rename(&temp_path, &final_path).map_err(|source| {
            drop(fs::remove_file(&temp_path));
            KernelError::io(format!("publishing {}", final_path.display()), source)
        })?;
        Ok(id)
    }

    fn get(&self, id: ObjId) -> Result<Option<Vec<u8>>> {
        let path = self.object_path(id);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(KernelError::io(
                format!("reading {}", path.display()),
                source,
            )),
        }
    }

    fn has(&self, id: ObjId) -> Result<bool> {
        Ok(self.object_path(id).exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(store: &dyn Cas) -> Result<()> {
        let id = store.put(b"payload")?;
        assert_eq!(id, ObjId::of(b"payload"));
        assert_eq!(store.get(id)?, Some(b"payload".to_vec()));
        assert!(store.has(id)?);
        // Absence is a `None`, not an error.
        assert_eq!(store.get(ObjId::of(b"absent"))?, None);
        assert!(!store.has(ObjId::of(b"absent"))?);
        // Re-putting is a no-op returning the same address.
        assert_eq!(store.put(b"payload")?, id);
        Ok(())
    }

    #[test]
    fn memory_store_round_trips() -> Result<()> {
        round_trip(&MemoryCas::new())
    }

    #[test]
    fn directory_store_round_trips() -> Result<()> {
        let dir = temp_dir("round-trip");
        let store = DirCas::open(&dir)?;
        let result = round_trip(&store);
        drop(fs::remove_dir_all(&dir));
        result
    }

    #[test]
    fn empty_object_is_storable() -> Result<()> {
        let store = MemoryCas::new();
        let id = store.put(b"")?;
        assert_eq!(store.get(id)?, Some(Vec::new()));
        assert_eq!(store.len(), 1);
        Ok(())
    }

    #[test]
    fn stored_objects_survive_reopening_the_directory() -> Result<()> {
        let dir = temp_dir("reopen");
        let id = DirCas::open(&dir)?.put(b"durable")?;
        let reopened = DirCas::open(&dir)?;
        let found = reopened.get(id)?;
        drop(fs::remove_dir_all(&dir));
        assert_eq!(found, Some(b"durable".to_vec()));
        Ok(())
    }

    /// Only object files, so a directory listing is a listing of addresses.
    #[test]
    fn writing_leaves_no_temporary_behind() -> Result<()> {
        let dir = temp_dir("no-temp");
        let store = DirCas::open(&dir)?;
        store.put(b"one")?;
        store.put(b"two")?;
        let mut names: Vec<String> = fs::read_dir(&dir)
            .map_err(|source| KernelError::io("listing", source))?
            .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
            .collect();
        names.sort();
        drop(fs::remove_dir_all(&dir));
        let mut expected = vec![
            ObjId::of(b"one").hash().to_hex(),
            ObjId::of(b"two").hash().to_hex(),
        ];
        expected.sort();
        assert_eq!(names, expected);
        Ok(())
    }

    fn temp_dir(label: &str) -> PathBuf {
        let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ix-kernel-cas-{label}-{}-{unique}",
            std::process::id()
        ))
    }
}
