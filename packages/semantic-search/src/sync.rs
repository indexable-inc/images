//! Dedup-aware sync. The deliberate departure from how mgrep works:
//!
//! 1. Files are addressed by content hash, so a blob already in the store is
//!    never re-uploaded or re-embedded. Twenty worktrees of one repo pay the
//!    embedding cost once.
//! 2. Sync never deletes. mgrep removes anything under the synced path that is
//!    absent locally, which silently wipes another worktree's files when two
//!    checkouts share a path. Because a stored entry is shared across
//!    checkouts, deletion can only be decided by reference counting across
//!    every manifest, so it lives in a separate garbage-collection pass, not
//!    in ordinary sync.
//!
//! There is no daemon. Each invocation rebuilds the manifest cheaply (mtime
//! skips re-hashing unchanged files) and uploads only what is new.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt as _};
use snafu::ResultExt as _;
use tokio::time::sleep;

use crate::backend::{Store, UploadMeta};
use crate::error::{ReadFileSnafu, Result, TooManyFilesSnafu};
use crate::manifest::{FileEntry, Manifest};

/// Maximum concurrent uploads in flight.
const UPLOAD_CONCURRENCY: usize = 16;

/// How often to poll the store for indexing progress.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Outcome of a sync pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    /// New blobs uploaded this run.
    pub uploaded: usize,
    /// Files already present in the store, skipped without an upload.
    pub skipped: usize,
    /// Total files in the manifest.
    pub total: usize,
}

/// Upload every file in `manifest` whose content is not already in `store`.
///
/// Returns once the uploads are accepted; call [`wait_until_indexed`] to block
/// until the new content is embedded and searchable.
///
/// # Errors
/// Returns an error if the store cannot be reached, if the number of new files
/// exceeds `max_files`, or if a file selected for upload cannot be read.
pub async fn sync(
    store: &(impl Store + Sync),
    store_name: &str,
    root: &Path,
    manifest: &Manifest,
    max_files: usize,
) -> Result<SyncReport> {
    store.ensure_store(store_name).await?;
    let remote = store.list_external_ids(store_name).await?;

    // New content only: skip hashes the store already has, and collapse
    // duplicate content within this checkout to a single upload.
    let mut seen = HashSet::new();
    let to_upload: Vec<&FileEntry> = manifest
        .entries
        .iter()
        .filter(|e| !remote.contains(e.hash.as_str()))
        .filter(|e| seen.insert(e.hash.as_str()))
        .collect();

    let total = manifest.entries.len();
    let upload_target = to_upload.len();
    let skipped = total - upload_target;

    if upload_target > max_files {
        return TooManyFilesSnafu {
            count: upload_target,
            max: max_files,
        }
        .fail();
    }

    let results: Vec<Result<()>> = stream::iter(to_upload)
        .map(|entry| async move {
            let abs = root.join(&entry.rel_path);
            let content = tokio::fs::read(&abs)
                .await
                .context(ReadFileSnafu { path: abs })?;
            let file_name = file_name_of(&entry.rel_path);
            let meta = UploadMeta {
                path: entry.rel_path.clone(),
                hash: entry.hash.as_str().to_owned(),
            };
            store
                .upload(store_name, content, file_name, entry.hash.as_str(), meta)
                .await
        })
        .buffer_unordered(UPLOAD_CONCURRENCY)
        .collect()
        .await;

    let mut uploaded = 0;
    for result in results {
        result?;
        uploaded += 1;
    }

    Ok(SyncReport {
        uploaded,
        skipped,
        total,
    })
}

/// Poll the store until newly uploaded files finish embedding.
///
/// Returns `true` when the store reports nothing pending or in progress, or
/// `false` if `timeout` elapses first (the caller can search anyway, accepting
/// possibly-incomplete results).
///
/// # Errors
/// Returns an error if a status request fails.
pub async fn wait_until_indexed(
    store: &(impl Store + Sync),
    store_name: &str,
    timeout: Duration,
) -> Result<bool> {
    let start = Instant::now();
    loop {
        let status = store.store_status(store_name).await?;
        if status.pending == 0 && status.in_progress == 0 {
            return Ok(true);
        }
        if start.elapsed() >= timeout {
            return Ok(false);
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn file_name_of(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

#[cfg(test)]
mod tests {
    use super::sync;
    use crate::backend::MemoryStore;
    use crate::manifest::Manifest;

    fn write_repo() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn a() {}").expect("write a");
        std::fs::write(dir.path().join("b.rs"), "fn b() {}").expect("write b");
        dir
    }

    #[tokio::test]
    async fn first_sync_uploads_all_then_second_uploads_nothing() {
        let dir = write_repo();
        let store = MemoryStore::new();
        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("manifest");

        let first = sync(&store, "s", dir.path(), &manifest, 1000)
            .await
            .expect("first sync");
        assert_eq!(first.uploaded, 2);
        assert_eq!(store.upload_count(), 2);

        // Re-syncing the same content must not re-upload: this is the
        // cross-worktree / re-run win that mgrep lacks.
        let second = sync(&store, "s", dir.path(), &manifest, 1000)
            .await
            .expect("second sync");
        assert_eq!(second.uploaded, 0);
        assert_eq!(second.skipped, 2);
        assert_eq!(store.upload_count(), 2, "no redundant re-upload");
    }

    #[tokio::test]
    async fn second_worktree_with_same_content_reuses_store() {
        let dir_a = write_repo();
        let dir_b = write_repo(); // identical content, different absolute path
        let store = MemoryStore::new();

        let manifest_a = Manifest::build(dir_a.path(), None, 1024 * 1024).expect("a");
        sync(&store, "s", dir_a.path(), &manifest_a, 1000)
            .await
            .expect("sync a");
        assert_eq!(store.upload_count(), 2);

        let manifest_b = Manifest::build(dir_b.path(), None, 1024 * 1024).expect("b");
        let report_b = sync(&store, "s", dir_b.path(), &manifest_b, 1000)
            .await
            .expect("sync b");

        assert_eq!(report_b.uploaded, 0, "identical worktree embeds nothing");
        assert_eq!(store.upload_count(), 2, "store still holds one copy");
        assert_eq!(store.len(), 2);
    }

    #[tokio::test]
    async fn over_limit_refuses_to_upload() {
        let dir = write_repo();
        let store = MemoryStore::new();
        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("manifest");

        let err = sync(&store, "s", dir.path(), &manifest, 1)
            .await
            .expect_err("should refuse");
        assert!(matches!(err, crate::error::Error::TooManyFiles { .. }));
        assert_eq!(store.upload_count(), 0);
    }
}
