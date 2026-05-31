//! The local manifest: this checkout's `relative path -> content hash` view,
//! held in memory.
//!
//! It drives two jobs. Sync compares it against the store's existing hashes to
//! decide what to upload. Search intersects results against it so a query only
//! surfaces files that exist in this worktree, even though byte-identical files
//! from other worktrees share one stored entry.
//!
//! Persistence lives in [`crate::db`]. Each entry records an mtime and size so
//! the next run reuses an unchanged file's hash instead of re-reading it; a
//! changed mtime forces a re-hash, so the skip is a cheap negative check that
//! never produces a wrong answer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use repo_walker::{FileScanner, WalkOptions};
use snafu::ResultExt as _;

use crate::content::ContentHash;
use crate::error::{ReadFileSnafu, Result, StatSnafu, WalkSnafu};

/// A file selected by the walk, awaiting a hash. Enumerating these is cheap and
/// sequential; computing their hashes is the expensive part and runs in
/// parallel.
struct Candidate {
    path: PathBuf,
    rel_path: String,
    mtime_ms: u64,
    size: u64,
}

/// One indexed file's identity and change-detection metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    /// Repo-relative path, forward-slashed and stable across worktrees.
    pub rel_path: String,
    /// Content hash, used as the store `external_id`.
    pub hash: ContentHash,
    /// Last-modified time in milliseconds since the Unix epoch.
    pub mtime_ms: u64,
    /// File size in bytes.
    pub size: u64,
}

/// A whole checkout's worth of [`FileEntry`] values, sorted by path.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// Indexed files, sorted by `rel_path` for stable output and diffs.
    pub entries: Vec<FileEntry>,
}

impl Manifest {
    /// Build a manifest by walking `root`, honoring gitignore and skipping
    /// binary and oversized files. When `previous` is supplied, an unchanged
    /// file (matching mtime and size) reuses its prior hash without re-reading.
    ///
    /// `root` should be absolute so the walker's paths and the stored relative
    /// paths line up.
    ///
    /// # Errors
    /// Returns an error if the walk fails, or if a selected file cannot be
    /// stat'd or read.
    pub fn build(root: &Path, previous: Option<&Self>, max_file_bytes: u64) -> Result<Self> {
        let prior: HashMap<&str, &FileEntry> = previous
            .map(|m| m.entries.iter().map(|e| (e.rel_path.as_str(), e)).collect())
            .unwrap_or_default();

        // Walk sequentially to enumerate candidates (cheap), then hash them in
        // parallel (the expensive read + sha256 work) across rayon's pool.
        let mut candidates = Vec::new();
        for item in FileScanner::new(root, WalkOptions::default()) {
            let path = item.context(WalkSnafu {
                root: root.to_path_buf(),
            })?;
            let metadata = std::fs::metadata(&path).context(StatSnafu { path: path.clone() })?;
            let size = metadata.len();
            if size == 0 || size > max_file_bytes {
                continue;
            }
            let mtime_ms = mtime_ms(&metadata);
            let rel_path = relative_path(root, &path);
            candidates.push(Candidate {
                path,
                rel_path,
                mtime_ms,
                size,
            });
        }

        let mut entries: Vec<FileEntry> = candidates
            .par_iter()
            .map(|candidate| -> Result<FileEntry> {
                let reuse = prior.get(candidate.rel_path.as_str()).filter(|prev| {
                    prev.mtime_ms == candidate.mtime_ms && prev.size == candidate.size
                });
                // Unchanged file: reuse its hash without reading from disk.
                let hash = if let Some(prev) = reuse {
                    prev.hash.clone()
                } else {
                    let bytes = std::fs::read(&candidate.path).context(ReadFileSnafu {
                        path: candidate.path.clone(),
                    })?;
                    ContentHash::of_bytes(&bytes)
                };
                Ok(FileEntry {
                    rel_path: candidate.rel_path.clone(),
                    hash,
                    mtime_ms: candidate.mtime_ms,
                    size: candidate.size,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(Self { entries })
    }

    /// The set of content hashes present in this checkout.
    #[must_use]
    pub fn hashes(&self) -> HashSet<&str> {
        self.entries.iter().map(|e| e.hash.as_str()).collect()
    }

    /// The first relative path that maps to `hash`, if any. Identical content
    /// at several paths is rare in practice; the first is a fine display label.
    #[must_use]
    pub fn path_for_hash(&self, hash: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.hash.as_str() == hash)
            .map(|e| e.rel_path.as_str())
    }
}

fn mtime_ms(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::Manifest;

    #[test]
    fn build_indexes_files_and_skips_binaries() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}").expect("write");
        std::fs::write(dir.path().join("b.txt"), "hello world").expect("write");
        std::fs::write(dir.path().join("image.png"), [0_u8, 1, 2, 3]).expect("write");

        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("build");
        let paths: Vec<&str> = manifest
            .entries
            .iter()
            .map(|e| e.rel_path.as_str())
            .collect();

        assert!(paths.contains(&"a.rs"));
        assert!(paths.contains(&"b.txt"));
        assert!(!paths.contains(&"image.png"), "binary extension skipped");
    }

    #[test]
    fn unchanged_file_reuses_prior_hash_object() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "fn main() {}").expect("write");

        let first = Manifest::build(dir.path(), None, 1024 * 1024).expect("first");
        let second = Manifest::build(dir.path(), Some(&first), 1024 * 1024).expect("second");

        assert_eq!(first.entries.len(), 1);
        assert_eq!(
            first.entries[0].hash, second.entries[0].hash,
            "stable content keeps a stable hash across builds"
        );
    }

    #[test]
    fn hashes_and_lookup_round_trip() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "alpha").expect("write");
        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("build");

        let hash = manifest.entries[0].hash.as_str().to_owned();
        assert!(manifest.hashes().contains(hash.as_str()));
        assert_eq!(manifest.path_for_hash(&hash), Some("a.rs"));
        assert_eq!(manifest.path_for_hash("sha256:nope"), None);
    }
}
