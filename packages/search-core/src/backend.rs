//! The storage backend abstraction.
//!
//! Sync and search are written against the [`Store`] trait so their logic is
//! tested against [`MemoryStore`] with no network, and the real
//! [`MixedbreadStore`](crate::MixedbreadStore) is the production
//! implementation.
//!
//! Trait methods return `impl Future + Send` rather than using `async fn`
//! sugar: a public `async fn` trait method forbids callers from adding the
//! `Send` bound the concurrent sync path needs, and the workspace denies that
//! warning. Implementors satisfy these with ordinary `async fn`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Mutex, PoisonError};

use regex::RegexBuilder;
use snafu::ResultExt as _;

use crate::error::{InvalidPatternSnafu, Result};

/// Metadata attached to every stored file.
///
/// The path is repo-relative so it is stable across worktrees; the hash equals
/// the `external_id` and lets a search result be mapped back through the local
/// manifest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UploadMeta {
    /// Repo-relative path, forward-slashed.
    pub path: String,
    /// Content hash, identical to the file's `external_id`.
    pub hash: String,
}

/// Knobs forwarded to the backend's search call.
#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    /// Apply the second-stage reranker for better ordering.
    pub rerank: bool,
    /// Let the backend plan and run several searches itself.
    pub agentic: bool,
}

/// Knobs forwarded to the backend's grep call.
#[derive(Debug, Clone, Copy)]
pub struct GrepOptions {
    /// Match the pattern case-sensitively. When false, the pattern matches
    /// regardless of case.
    pub case_sensitive: bool,
    /// Which chunk field(s) the pattern is matched against.
    pub targets: GrepTargets,
}

/// Which indexed field a grep pattern is matched against.
#[derive(Debug, Clone, Copy, Default)]
pub enum GrepTargets {
    /// Match the chunk's raw text (the source content).
    #[default]
    Text,
    /// Match the chunk's generated metadata text.
    Generated,
}

impl GrepTargets {
    /// The API target strings for this selection: `Text` maps to `["text"]`,
    /// `Generated` maps to `["generated"]`. Returned as a borrowed slice of
    /// static strings so it can be passed straight to the client's `grep` call.
    #[must_use]
    pub const fn api_targets(self) -> &'static [&'static str] {
        match self {
            Self::Text => &["text"],
            Self::Generated => &["generated"],
        }
    }
}

/// One scored chunk returned by a search.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Content hash from the stored metadata, when present. Absent for web
    /// results.
    pub hash: Option<String>,
    /// Path or URL reported by the backend.
    pub path: Option<String>,
    /// The matched text snippet.
    pub text: String,
    /// Relevance score in `0.0..=1.0`.
    pub score: f32,
    /// Zero-based start line of the chunk within its file, when known.
    pub start_line: Option<u32>,
    /// Number of lines in the chunk (a count, not a span), when known.
    pub num_lines: Option<u32>,
}

/// A question-answering response: a synthesized answer plus its sources.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The synthesized answer text.
    pub answer: String,
    /// Chunks the answer drew from.
    pub sources: Vec<SearchHit>,
}

/// Indexing progress for a store. Zero on both fields means everything
/// uploaded so far has been embedded and is searchable.
#[derive(Debug, Clone, Copy)]
pub struct StoreStatus {
    /// Files queued but not yet processed.
    pub pending: u64,
    /// Files currently being embedded.
    pub in_progress: u64,
}

/// A vector store that holds content-addressed files and answers searches.
pub trait Store {
    /// Ensure the named store exists, creating it if absent.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or the store cannot
    /// be created.
    fn ensure_store(&self, name: &str) -> impl Future<Output = Result<()>> + Send;

    /// List the `external_id`s already present in the store. These are content
    /// hashes; sync uploads only the ones missing from this set.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or returns an error
    /// status.
    fn list_external_ids(
        &self,
        store: &str,
    ) -> impl Future<Output = Result<HashSet<String>>> + Send;

    /// Upload one file's content under the given `external_id` (its hash).
    ///
    /// # Errors
    /// Returns an error if the upload fails at either the file or attach step.
    fn upload(
        &self,
        store: &str,
        content: Vec<u8>,
        file_name: &str,
        external_id: &str,
        meta: UploadMeta,
    ) -> impl Future<Output = Result<()>> + Send;

    /// Delete one file by `external_id`. Used only by an explicit garbage
    /// collection pass, never by ordinary sync.
    ///
    /// # Errors
    /// Returns an error if the delete request fails.
    fn delete(&self, store: &str, external_id: &str) -> impl Future<Output = Result<()>> + Send;

    /// Search one or more stores for a query.
    ///
    /// # Errors
    /// Returns an error if the search request fails or the response cannot be
    /// decoded.
    fn search(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
    ) -> impl Future<Output = Result<Vec<SearchHit>>> + Send;

    /// Grep one or more stores with a regular expression over the same chunks
    /// search covers. `pattern` is the regex, `top_k` caps the matches, and
    /// `options` carries case sensitivity and the matched target field.
    ///
    /// # Errors
    /// Returns an error if the pattern is not a valid regular expression, or if
    /// the grep request fails or the response cannot be decoded.
    fn grep(
        &self,
        stores: &[String],
        pattern: &str,
        top_k: usize,
        options: GrepOptions,
    ) -> impl Future<Output = Result<Vec<SearchHit>>> + Send;

    /// Ask a natural-language question against one or more stores.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
    ) -> impl Future<Output = Result<Answer>> + Send;

    /// Fetch indexing progress for a store, used to wait until newly uploaded
    /// files are embedded and searchable.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn store_status(&self, store: &str) -> impl Future<Output = Result<StoreStatus>> + Send;
}

/// In-memory [`Store`] for tests.
///
/// Keyed by `external_id` like the real store, so the dedup behavior (a
/// repeated hash is a no-op) is exercised faithfully. It also counts upload
/// calls so tests can assert nothing was re-uploaded.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    files: HashMap<String, StoredFile>,
    upload_calls: usize,
}

#[derive(Debug)]
struct StoredFile {
    content: Vec<u8>,
    meta: UploadMeta,
}

impl MemoryStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many times [`Store::upload`] actually ran. Tests assert this stays
    /// flat across redundant syncs.
    #[must_use]
    pub fn upload_count(&self) -> usize {
        self.lock().upload_calls
    }

    /// How many distinct files are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().files.len()
    }

    /// Whether the store holds no files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().files.is_empty()
    }
}

impl Store for MemoryStore {
    async fn ensure_store(&self, _name: &str) -> Result<()> {
        Ok(())
    }

    async fn list_external_ids(&self, _store: &str) -> Result<HashSet<String>> {
        Ok(self.lock().files.keys().cloned().collect())
    }

    async fn upload(
        &self,
        _store: &str,
        content: Vec<u8>,
        _file_name: &str,
        external_id: &str,
        meta: UploadMeta,
    ) -> Result<()> {
        let mut inner = self.lock();
        inner.upload_calls += 1;
        inner
            .files
            .insert(external_id.to_owned(), StoredFile { content, meta });
        drop(inner);
        Ok(())
    }

    async fn delete(&self, _store: &str, external_id: &str) -> Result<()> {
        self.lock().files.remove(external_id);
        Ok(())
    }

    async fn search(
        &self,
        _stores: &[String],
        query: &str,
        top_k: usize,
        _options: SearchOptions,
    ) -> Result<Vec<SearchHit>> {
        let needle = query.to_lowercase();
        let inner = self.lock();
        let mut hits = Vec::new();
        for file in inner.files.values() {
            let text = String::from_utf8_lossy(&file.content);
            for (index, line) in text.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    hits.push(SearchHit {
                        hash: Some(file.meta.hash.clone()),
                        path: Some(file.meta.path.clone()),
                        text: line.to_owned(),
                        score: 1.0,
                        start_line: u32::try_from(index).ok(),
                        num_lines: Some(1),
                    });
                }
                if hits.len() >= top_k {
                    break;
                }
            }
            if hits.len() >= top_k {
                break;
            }
        }
        drop(inner);
        Ok(hits)
    }

    async fn grep(
        &self,
        _stores: &[String],
        pattern: &str,
        top_k: usize,
        options: GrepOptions,
    ) -> Result<Vec<SearchHit>> {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(!options.case_sensitive)
            .build()
            .with_context(|_| InvalidPatternSnafu {
                pattern: pattern.to_owned(),
            })?;

        let inner = self.lock();
        let mut hits = Vec::new();
        for file in inner.files.values() {
            let text = String::from_utf8_lossy(&file.content);
            for (index, line) in text.lines().enumerate() {
                if regex.is_match(line) {
                    hits.push(SearchHit {
                        hash: Some(file.meta.hash.clone()),
                        path: Some(file.meta.path.clone()),
                        text: line.to_owned(),
                        score: 1.0,
                        start_line: u32::try_from(index).ok(),
                        num_lines: Some(1),
                    });
                }
                if hits.len() >= top_k {
                    break;
                }
            }
            if hits.len() >= top_k {
                break;
            }
        }
        drop(inner);
        Ok(hits)
    }

    async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
    ) -> Result<Answer> {
        let sources = self.search(stores, query, top_k, options).await?;
        Ok(Answer {
            answer: "mock answer from MemoryStore".to_owned(),
            sources,
        })
    }

    async fn store_status(&self, _store: &str) -> Result<StoreStatus> {
        // In-memory uploads are immediately "indexed".
        Ok(StoreStatus {
            pending: 0,
            in_progress: 0,
        })
    }
}
