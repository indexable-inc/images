//! High-level orchestration: build the manifest, embed any new files, then
//! search, grep, or answer in one call. The CLI and the `PyO3` bindings both go
//! through here, so the index-then-query flow lives in exactly one place.

use std::path::Path;
use std::time::Duration;

use mixedbread::Filter;

use crate::backend::{GrepOptions, SearchOptions, Store, StoreStatus};
use crate::config::Config;
use crate::db::Db;
use crate::error::Result;
use crate::manifest::Manifest;
use crate::repo::repo_slug;
use crate::search::{AnswerView, CodeScope, DisplayHit, ask, grep, semantic};
use crate::sync::{sync, wait_until_indexed};

/// What to query and how, independent of the backend and progress reporting.
#[derive(Debug, Clone, Copy)]
pub struct Query<'a> {
    /// Absolute checkout root to index and scope results to.
    pub root: &'a Path,
    /// Store name (one store holds every worktree's content).
    pub store_name: &'a str,
    /// API base URL of the backend the store lives on. Part of the store's
    /// identity: the same store name on two different endpoints is two different
    /// stores, so the "already synced" gate must distinguish them or it would
    /// skip uploading to a freshly pointed-at instance.
    pub base_url: &'a str,
    /// The query text: a natural-language query for semantic search, or a
    /// regular expression for grep.
    pub text: &'a str,
    /// Maximum results to return.
    pub top_k: usize,
    /// Search tuning (rerank, agentic).
    pub options: SearchOptions,
    /// Detect and embed new files before querying. Set false for offline search.
    pub sync: bool,
    /// Mix in web results.
    pub include_web: bool,
    /// Metadata filter applied server-side (source/repo/channel/...). `None`
    /// means all sources.
    pub filters: Option<&'a Filter>,
    /// How code hits are scoped: worktree-exact (manifest intersection) or
    /// server-filtered (a repo / all-repos query).
    pub code_scope: CodeScope,
    /// How long to wait for new files to embed before querying anyway.
    pub index_timeout: Duration,
}

/// Build + persist the manifest for the checkout, and (when `query.sync`)
/// upload new content and wait for it to embed. Returns the manifest used to
/// scope results.
async fn prepare(
    store: &(impl Store + Sync),
    query: &Query<'_>,
    config: &Config,
    on_upload: impl Fn(usize, usize) + Send + Sync,
    on_poll: impl Fn(StoreStatus) + Send + Sync,
) -> Result<Manifest> {
    // Identity of the store this checkout is synced against: the same store name
    // on a different API endpoint is a different store, so both must key the
    // "already synced" gate, or pointing at a fresh instance would be skipped.
    let synced_store = format!("{}\u{1f}{}", query.base_url, query.store_name);

    // Scope the database handle so it is dropped before any await: rusqlite's
    // connection is not Sync, and the returned future must be Send.
    let (manifest, signature, already_synced) = {
        let mut db = Db::open()?;
        let previous = db.load(query.root)?;
        let manifest = Manifest::build(query.root, Some(&previous), config.max_file_bytes)?;
        db.save(query.root, &manifest)?;
        let signature = manifest.signature();
        // If this exact content was already synced to this store, skip the sync
        // round-trips entirely. Code sync never deletes (the module is
        // append-only; deletion is a separate GC pass), so an unchanged
        // signature means every blob is still present and listing the store to
        // rediscover that is pure latency. This is what turns a repeated search
        // on an already-indexed checkout from "re-list every file" into a no-op.
        let already_synced = db
            .synced_signature(query.root, &synced_store)?
            .as_deref()
            == Some(signature.as_str());
        (manifest, signature, already_synced)
    };

    if query.sync && !already_synced {
        let repo = repo_slug(query.root);
        let report = sync(
            store,
            query.store_name,
            query.root,
            &manifest,
            &repo,
            config.max_files,
            on_upload,
        )
        .await?;
        if report.uploaded > 0 {
            wait_until_indexed(store, query.store_name, query.index_timeout, on_poll).await?;
        }
        // Record success once the uploads are accepted, not once embedding
        // finishes. Upload acceptance is the durable fact the gate cares about
        // (the blobs are in the store, addressed by hash); embedding completes
        // asynchronously server-side. Gating the mark on embedding instead would
        // force the full repo re-list on every run until a slow embed catches
        // up, and a re-upload of identical content cannot fix a server-side
        // embed failure anyway. Open a fresh handle: the one above was dropped
        // before this await to keep the future Send.
        Db::open()?.mark_synced(query.root, &synced_store, &signature)?;
    }

    Ok(manifest)
}

/// Index the checkout (unless `query.sync` is false) and return search hits
/// scoped to it.
///
/// # Errors
/// Returns an error if indexing or the search request fails.
pub async fn index_and_semantic(
    store: &(impl Store + Sync),
    query: &Query<'_>,
    config: &Config,
    on_upload: impl Fn(usize, usize) + Send + Sync,
    on_poll: impl Fn(StoreStatus) + Send + Sync,
) -> Result<Vec<DisplayHit>> {
    let manifest = prepare(store, query, config, on_upload, on_poll).await?;
    semantic(
        store,
        query.store_name,
        &manifest,
        query.text,
        query.top_k,
        query.options,
        query.include_web,
        query.filters,
        query.code_scope,
    )
    .await
}

/// Index the checkout (unless `query.sync` is false) and return regex grep hits.
///
/// The pattern is `query.text`; `options` carries case sensitivity and the
/// matched target field. Grep is local-corpus only, so `query.options` and
/// `query.include_web` are ignored here.
///
/// # Errors
/// Returns an error if indexing fails, the pattern is not a valid regular
/// expression, or the grep request fails.
pub async fn index_and_grep(
    store: &(impl Store + Sync),
    query: &Query<'_>,
    options: GrepOptions,
    config: &Config,
    on_upload: impl Fn(usize, usize) + Send + Sync,
    on_poll: impl Fn(StoreStatus) + Send + Sync,
) -> Result<Vec<DisplayHit>> {
    let manifest = prepare(store, query, config, on_upload, on_poll).await?;
    grep(
        store,
        query.store_name,
        &manifest,
        query.text,
        query.top_k,
        options,
        query.filters,
        query.code_scope,
    )
    .await
}

/// Index the checkout (unless `query.sync` is false) and return a synthesized
/// answer with sources scoped to it.
///
/// # Errors
/// Returns an error if indexing or the question-answering request fails.
pub async fn index_and_answer(
    store: &(impl Store + Sync),
    query: &Query<'_>,
    config: &Config,
    on_upload: impl Fn(usize, usize) + Send + Sync,
    on_poll: impl Fn(StoreStatus) + Send + Sync,
) -> Result<AnswerView> {
    let manifest = prepare(store, query, config, on_upload, on_poll).await?;
    ask(
        store,
        query.store_name,
        &manifest,
        query.text,
        query.top_k,
        query.options,
        query.include_web,
        query.filters,
        query.code_scope,
    )
    .await
}
