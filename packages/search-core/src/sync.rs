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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt as _};
use mixedbread::Filter;
use source_meta::{Document, RepoSlug, keys};
use snafu::ResultExt as _;
use tokio::time::sleep;

use crate::backend::{Store, StoreStatus};
use crate::error::{MetadataLimitSnafu, ReadFileSnafu, Result, TooManyFilesSnafu};
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
/// `on_progress(uploaded_so_far, total_to_upload)` is called once with the
/// total before uploading and again after each successful upload, so a caller
/// can render a progress bar. It may run from several concurrent upload tasks.
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
    repo: &RepoSlug,
    max_files: usize,
    on_progress: impl Fn(usize, usize) + Send + Sync,
) -> Result<SyncReport> {
    store.ensure_store(store_name).await?;

    // Scope the "what's already there" listing to this repo's code records.
    // The shared store also holds every other repo, every worktree, and the
    // non-code sources (Slack, Linear, ...); listing it unfiltered means one
    // sync paginates the whole world before a single byte uploads, which is the
    // dominant first-run stall on an established store.
    //
    // A blob is addressed by its content hash and upload overwrites by that id,
    // so a too-narrow scope never duplicates or corrupts: the worst case is a
    // file whose content is byte-identical across two repos getting re-uploaded
    // (a cheap, idempotent overwrite) the first time each repo syncs it. The
    // default worktree search intersects by content hash and is unaffected; only
    // a `--repo`/`--all-worktrees` query for such a shared blob sees its repo
    // attribution follow the most recent sync. That is rare (shared content is
    // usually boilerplate) and was already arbitrary under the unfiltered list.
    let scope = Filter::all(vec![
        Filter::eq(keys::SOURCE, source_meta::Source::code().as_str()),
        Filter::eq(keys::REPO, repo.as_str()),
    ]);
    let remote = store.list_external_ids(store_name, Some(&scope)).await?;

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

    on_progress(0, upload_target);
    let done = AtomicUsize::new(0);

    // Feed owned `FileEntry` clones into the upload stream rather than the
    // `&FileEntry` references gathered above. A per-task future that borrows its
    // entry makes the stream's closure return `fn(&'0 FileEntry) -> impl Future`,
    // whose higher-ranked lifetime a `Send + 'static` consumer cannot unify
    // (`implementation of FnOnce is not general enough`). The PyO3 bindings hit
    // exactly this: they drive `index_and_search` through
    // `pyo3_async_runtimes::future_into_py`, which requires the whole future to
    // be `Send + 'static`. Owning the entry per task removes the borrow and the
    // clone is one small struct per uploaded file, paid only for new content.
    let to_upload: Vec<FileEntry> = to_upload.into_iter().cloned().collect();
    let ctx = UploadContext {
        store,
        store_name,
        root,
        repo,
        upload_target,
        done: &done,
        on_progress: &on_progress,
    };
    let results: Vec<Result<()>> = stream::iter(to_upload)
        .map(|entry| upload_entry(&ctx, entry))
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

/// Shared, per-run context borrowed by every [`upload_entry`] task. Bundling the
/// invariant uploader state keeps the per-entry future a named, concrete type
/// (see the [`sync`] call site for why an inline closure breaks higher-ranked
/// lifetime unification for `Send + 'static` consumers) without threading eight
/// separate arguments through each task.
struct UploadContext<'a, S: Store + Sync, P: Fn(usize, usize) + Send + Sync> {
    store: &'a S,
    store_name: &'a str,
    root: &'a Path,
    repo: &'a RepoSlug,
    upload_target: usize,
    done: &'a AtomicUsize,
    on_progress: &'a P,
}

/// Read one file and upload it under its content hash, then report progress.
async fn upload_entry<S: Store + Sync, P: Fn(usize, usize) + Send + Sync>(
    ctx: &UploadContext<'_, S, P>,
    entry: FileEntry,
) -> Result<()> {
    let abs = ctx.root.join(&entry.rel_path);
    let content = tokio::fs::read(&abs)
        .await
        .context(ReadFileSnafu { path: abs })?;
    let document = code_document(ctx.repo, &entry, content)?;
    ctx.store.upload(ctx.store_name, document).await?;
    (ctx.on_progress)(ctx.done.fetch_add(1, Ordering::Relaxed) + 1, ctx.upload_target);
    Ok(())
}

/// Build the [`Document`] for one code file: the file bytes are the body, the
/// content hash is the manifest hash (sha256 of those bytes) and doubles as the
/// `external_id`, and the flat metadata carries the typed code envelope so a
/// query can filter by `source` and `repo`.
fn code_document(repo: &RepoSlug, entry: &FileEntry, body: Vec<u8>) -> Result<Document> {
    let hash = entry.hash.as_str().to_owned();
    let meta_json = serde_json::json!({
        "source": "code",
        "external_id": hash,
        "content_hash": hash,
        "title": entry.rel_path,
        "repo": repo.as_str(),
        "path": entry.rel_path,
    });
    source_meta::check_metadata(&hash, &meta_json).context(MetadataLimitSnafu)?;
    Ok(Document {
        external_id: hash.clone(),
        file_name: file_name_of(&entry.rel_path).to_owned(),
        mime: "text/plain",
        body,
        meta_json,
        content_hash: hash,
    })
}

/// Poll the store until newly uploaded files finish embedding.
///
/// `on_poll(status)` is called with each observed status so a caller can show
/// progress. Returns `true` when the store reports nothing pending or in
/// progress, or `false` if `timeout` elapses first (the caller can search
/// anyway, accepting possibly-incomplete results).
///
/// # Errors
/// Returns an error if a status request fails.
pub async fn wait_until_indexed(
    store: &(impl Store + Sync),
    store_name: &str,
    timeout: Duration,
    on_poll: impl Fn(StoreStatus) + Send + Sync,
) -> Result<bool> {
    let start = Instant::now();
    loop {
        let status = store.store_status(store_name).await?;
        on_poll(status);
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


    use source_meta::RepoSlug;

    use super::sync;
    use crate::backend::MemoryStore;
    use crate::manifest::Manifest;

    fn repo() -> RepoSlug {
        RepoSlug::Local("test".to_owned())
    }

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

        let first = sync(&store, "s", dir.path(), &manifest, &repo(), 1000, |_, _| {})
            .await
            .expect("first sync");
        assert_eq!(first.uploaded, 2);
        assert_eq!(store.upload_count(), 2);

        // Re-syncing the same content must not re-upload: this is the
        // cross-worktree / re-run win that mgrep lacks.
        let second = sync(&store, "s", dir.path(), &manifest, &repo(), 1000, |_, _| {})
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
        sync(&store, "s", dir_a.path(), &manifest_a, &repo(), 1000, |_, _| {})
            .await
            .expect("sync a");
        assert_eq!(store.upload_count(), 2);

        let manifest_b = Manifest::build(dir_b.path(), None, 1024 * 1024).expect("b");
        let report_b = sync(&store, "s", dir_b.path(), &manifest_b, &repo(), 1000, |_, _| {})
            .await
            .expect("sync b");

        assert_eq!(report_b.uploaded, 0, "identical worktree embeds nothing");
        assert_eq!(store.upload_count(), 2, "store still holds one copy");
        assert_eq!(store.len(), 2);
    }

    #[tokio::test]
    async fn dedup_listing_is_scoped_per_repo() {
        // The dedup listing is scoped to the syncing repo, so a blob already in
        // the store under a *different* repo is not seen and is uploaded again
        // (an idempotent overwrite, keyed by content hash). This is the cost of
        // not paginating the whole shared store on every sync; the win is that
        // one repo's sync never reads another repo's (or Slack's, Linear's...)
        // records.
        let dir = write_repo();
        let store = MemoryStore::new();
        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("manifest");
        let repo_a = RepoSlug::Remote("org/a".to_owned());
        let repo_b = RepoSlug::Remote("org/b".to_owned());

        let a = sync(&store, "s", dir.path(), &manifest, &repo_a, 1000, |_, _| {})
            .await
            .expect("sync a");
        assert_eq!(a.uploaded, 2);

        // Identical content under repo B: B's repo-scoped listing does not see
        // A's copies, so B re-uploads. The store still holds one entry per hash.
        let b = sync(&store, "s", dir.path(), &manifest, &repo_b, 1000, |_, _| {})
            .await
            .expect("sync b");
        assert_eq!(b.uploaded, 2, "different repo re-uploads identical content");
        assert_eq!(store.len(), 2, "overwrite by content hash, no duplicates");

        // A second B sync is a no-op: its own records now match the scope.
        let b_again = sync(&store, "s", dir.path(), &manifest, &repo_b, 1000, |_, _| {})
            .await
            .expect("sync b again");
        assert_eq!(b_again.uploaded, 0);
        assert_eq!(b_again.skipped, 2);
    }

    #[tokio::test]
    async fn over_limit_refuses_to_upload() {
        let dir = write_repo();
        let store = MemoryStore::new();
        let manifest = Manifest::build(dir.path(), None, 1024 * 1024).expect("manifest");

        let err = sync(&store, "s", dir.path(), &manifest, &repo(), 1, |_, _| {})
            .await
            .expect_err("should refuse");
        assert!(matches!(err, crate::error::Error::TooManyFiles { .. }));
        assert_eq!(store.upload_count(), 0);
    }
}
