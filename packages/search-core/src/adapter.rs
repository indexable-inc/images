//! Adapter wiring the standalone [`mixedbread::Client`] to this crate's
//! backend-agnostic [`Store`] trait. It maps a [`Document`] to the client's
//! upload call on the way in, and the client's [`mixedbread::Chunk`] to the
//! domain [`SearchHit`] on the way out, reading the typed envelope's `source`
//! and `content_hash` from each chunk's metadata.

use mixedbread::Filter;
use snafu::ResultExt as _;
use source_meta::{Document, Source};

use crate::backend::{
    Answer, GrepOptions, SearchHit, SearchOptions, Store, StoreStatus, StoredRecord,
};
use crate::error::{BackendSnafu, Result};

/// A [`Store`] backed by the Mixedbread API.
#[derive(Debug, Clone)]
pub struct MixedbreadStore {
    client: mixedbread::Client,
}

impl MixedbreadStore {
    /// Wrap an already-built client.
    #[must_use]
    pub const fn new(client: mixedbread::Client) -> Self {
        Self { client }
    }

    /// Build a store from the API key in the environment.
    ///
    /// # Errors
    /// Returns an error if the API key is missing or the client cannot be built.
    pub fn from_env(base_url: impl Into<String>) -> Result<Self> {
        let client = mixedbread::Client::from_env(base_url).context(BackendSnafu)?;
        Ok(Self { client })
    }

    /// Build a store resolving the credential from `MXBAI_API_KEY` or, failing
    /// that, the token stored by `mgrep login`.
    ///
    /// # Errors
    /// Returns an error if no credential can be resolved or the client cannot
    /// be built.
    pub async fn from_login(base_url: impl Into<String>) -> Result<Self> {
        let client = mixedbread::Client::from_login(base_url)
            .await
            .context(BackendSnafu)?;
        Ok(Self { client })
    }
}

fn to_client_options(options: SearchOptions) -> mixedbread::SearchOptions {
    mixedbread::SearchOptions {
        rerank: options.rerank,
        agentic: options.agentic,
        score_threshold: None,
        // Always request metadata so each hit's `source` and `content_hash` come
        // back; the projection needs `source` to scope correctly.
        return_metadata: Some(true),
    }
}

fn hit_from_chunk(chunk: mixedbread::Chunk) -> SearchHit {
    let metadata = chunk.metadata.as_ref();
    // Legacy code records (uploaded before the typed envelope) carry `hash`/`path`
    // and no `source`; the old store was code-only, so an absent source means
    // code. A present source tag is preserved verbatim (any corpus, open set).
    let source =
        metadata_str(metadata, source_meta::keys::SOURCE).map_or_else(Source::code, Source::from);
    let hash = metadata_str(metadata, source_meta::keys::CONTENT_HASH)
        .or_else(|| metadata_str(metadata, "hash"));
    // Code records carry `path`; record sources carry `title`. Either is the
    // display label.
    let path_meta = metadata_str(metadata, source_meta::keys::PATH)
        .or_else(|| metadata_str(metadata, source_meta::keys::TITLE));
    // The mixedbread API reports `start_line` 1-based and `num_lines` as a line
    // span (end - start), so an N-line chunk arrives as (start=1, num=N-1). The
    // rest of this crate uses a 0-based start and a line count, so normalize at
    // this boundary: shift the start down by one and turn the span into a count.
    SearchHit {
        source,
        hash,
        path: path_meta.or(chunk.filename),
        text: chunk.text.unwrap_or_default(),
        score: chunk.score,
        start_line: chunk.start_line.map(|line| line.saturating_sub(1)),
        num_lines: chunk.num_lines.map(|span| span.saturating_add(1)),
    }
}

fn metadata_str(metadata: Option<&serde_json::Value>, key: &str) -> Option<String> {
    metadata
        .and_then(|m| m.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

impl Store for MixedbreadStore {
    async fn ensure_store(&self, name: &str) -> Result<()> {
        self.client.ensure_store(name).await.context(BackendSnafu)
    }

    async fn list_external_ids(
        &self,
        store: &str,
        filters: Option<&Filter>,
    ) -> Result<std::collections::HashSet<String>> {
        let files = self
            .client
            .list_files(store, filters)
            .await
            .context(BackendSnafu)?;
        Ok(files
            .into_iter()
            .filter_map(|file| file.external_id)
            .collect())
    }

    async fn list_records(
        &self,
        store: &str,
        filters: Option<&Filter>,
    ) -> Result<Vec<StoredRecord>> {
        let files = self
            .client
            .list_files(store, filters)
            .await
            .context(BackendSnafu)?;
        Ok(files
            .into_iter()
            .filter_map(|file| {
                file.external_id.map(|external_id| StoredRecord {
                    content_hash: metadata_str(
                        file.metadata.as_ref(),
                        source_meta::keys::CONTENT_HASH,
                    ),
                    source: metadata_str(file.metadata.as_ref(), source_meta::keys::SOURCE),
                    external_id,
                })
            })
            .collect())
    }

    async fn upload(&self, store: &str, document: Document) -> Result<()> {
        self.client
            .upload_file(
                store,
                document.body,
                &document.file_name,
                &document.external_id,
                document.mime,
                document.meta_json,
            )
            .await
            .context(BackendSnafu)
    }

    async fn delete(&self, store: &str, external_id: &str) -> Result<()> {
        self.client
            .delete_file(store, external_id)
            .await
            .context(BackendSnafu)
    }

    async fn search(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&Filter>,
    ) -> Result<Vec<SearchHit>> {
        let chunks = self
            .client
            .search(stores, query, top_k, to_client_options(options), filters)
            .await
            .context(BackendSnafu)?;
        Ok(chunks.into_iter().map(hit_from_chunk).collect())
    }

    async fn grep(
        &self,
        stores: &[String],
        pattern: &str,
        top_k: usize,
        options: GrepOptions,
        filters: Option<&Filter>,
    ) -> Result<Vec<SearchHit>> {
        let chunks = self
            .client
            .grep(
                stores,
                pattern,
                top_k,
                options.case_sensitive,
                options.targets.api_targets(),
                filters,
            )
            .await
            .context(BackendSnafu)?;
        Ok(chunks.into_iter().map(hit_from_chunk).collect())
    }

    async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&Filter>,
    ) -> Result<Answer> {
        let response = self
            .client
            .ask(stores, query, top_k, to_client_options(options), filters)
            .await
            .context(BackendSnafu)?;
        Ok(Answer {
            answer: response.answer,
            sources: response.sources.into_iter().map(hit_from_chunk).collect(),
        })
    }

    async fn store_status(&self, store: &str) -> Result<StoreStatus> {
        let status = self
            .client
            .store_status(store)
            .await
            .context(BackendSnafu)?;
        Ok(StoreStatus {
            pending: status.pending,
            in_progress: status.in_progress,
        })
    }
}
