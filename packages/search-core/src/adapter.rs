//! Adapter wiring the standalone [`mixedbread::Client`] to this crate's
//! backend-agnostic [`Store`] trait. It does the only impedance matching
//! needed: typed [`UploadMeta`] to JSON on the way in, and the client's
//! [`mixedbread::Chunk`] to the domain [`SearchHit`] on the way out.

use std::collections::HashSet;

use snafu::ResultExt as _;

use crate::backend::{
    Answer, GrepOptions, SearchHit, SearchOptions, Store, StoreStatus, UploadMeta,
};
use crate::error::{BackendSnafu, EncodeMetadataSnafu, Result};

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

const fn to_client_options(options: SearchOptions) -> mixedbread::SearchOptions {
    mixedbread::SearchOptions {
        rerank: options.rerank,
        agentic: options.agentic,
    }
}

fn hit_from_chunk(chunk: mixedbread::Chunk) -> SearchHit {
    let hash = metadata_str(chunk.metadata.as_ref(), "hash");
    let path_meta = metadata_str(chunk.metadata.as_ref(), "path");
    // The mixedbread API reports `start_line` 1-based and `num_lines` as a line
    // span (end - start), so an N-line chunk arrives as (start=1, num=N-1). The
    // rest of this crate (and the local backend) use a 0-based start and a line
    // count, so normalize at this boundary: shift the start down by one and turn
    // the span into a count. Without this the renderer drops the chunk's first
    // line and empties single-line chunks, which then fall back to plain text.
    SearchHit {
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

    async fn list_external_ids(&self, store: &str) -> Result<HashSet<String>> {
        self.client
            .list_external_ids(store)
            .await
            .context(BackendSnafu)
    }

    async fn upload(
        &self,
        store: &str,
        content: Vec<u8>,
        file_name: &str,
        external_id: &str,
        meta: UploadMeta,
    ) -> Result<()> {
        let metadata = serde_json::to_value(&meta).context(EncodeMetadataSnafu)?;
        self.client
            .upload_file(store, content, file_name, external_id, metadata)
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
    ) -> Result<Vec<SearchHit>> {
        let chunks = self
            .client
            .search(stores, query, top_k, to_client_options(options))
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
    ) -> Result<Vec<SearchHit>> {
        let chunks = self
            .client
            .grep(
                stores,
                pattern,
                top_k,
                options.case_sensitive,
                options.targets.api_targets(),
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
    ) -> Result<Answer> {
        let response = self
            .client
            .ask(stores, query, top_k, to_client_options(options))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(start_line: Option<u32>, num_lines: Option<u32>) -> mixedbread::Chunk {
        mixedbread::Chunk {
            text: Some("body".to_owned()),
            score: 0.5,
            filename: Some("a.rs".to_owned()),
            start_line,
            num_lines,
            metadata: None,
        }
    }

    #[test]
    fn normalizes_one_based_span_to_zero_based_count() {
        // The API's (start=1, span=N-1) for an N-line chunk becomes the internal
        // (start=0, count=N): a whole 6-line file reported as (1, 5).
        let hit = hit_from_chunk(chunk(Some(1), Some(5)));
        assert_eq!(hit.start_line, Some(0));
        assert_eq!(hit.num_lines, Some(6));

        // A single-line chunk arrives as (start=1, span=0) and must become a
        // count of 1 line, not 0, so the renderer emits it instead of an empty
        // window that falls back to plain text.
        let single = hit_from_chunk(chunk(Some(1), Some(0)));
        assert_eq!(single.start_line, Some(0));
        assert_eq!(single.num_lines, Some(1));
    }

    #[test]
    fn missing_line_metadata_stays_none() {
        let hit = hit_from_chunk(chunk(None, None));
        assert_eq!(hit.start_line, None);
        assert_eq!(hit.num_lines, None);
    }
}
