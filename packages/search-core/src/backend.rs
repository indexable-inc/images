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

use mixedbread::{Condition, Filter, Operator};
use regex::RegexBuilder;
use search_meta::{Document, Source};
use snafu::{OptionExt as _, ResultExt as _};

use crate::error::{InvalidMetadataSnafu, InvalidPatternSnafu, Result};

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
    /// `Generated` maps to `["generated"]`.
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
    /// Which corpus the hit came from.
    pub source: Source,
    /// Content hash from the stored metadata, when present. Code hits carry it
    /// (it is the manifest key); web hits do not.
    pub hash: Option<String>,
    /// Path, title, or URL reported by the backend, for display.
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

/// One record's identity and change-detection hash, as listed from the store
/// during a per-source reconcile.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    /// The record's `external_id`.
    pub external_id: String,
    /// The `content_hash` stored in its metadata, when present. Absent means the
    /// record predates content-hash tracking and must be treated as changed.
    pub content_hash: Option<String>,
}

/// A vector store that holds documents and answers searches.
pub trait Store {
    /// Ensure the named store exists, creating it if absent.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or the store cannot be
    /// created.
    fn ensure_store(&self, name: &str) -> impl Future<Output = Result<()>> + Send;

    /// List the `external_id`s already present in the store. Used by the code
    /// sync path, where the id is the content hash.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or returns an error
    /// status.
    fn list_external_ids(
        &self,
        store: &str,
    ) -> impl Future<Output = Result<HashSet<String>>> + Send;

    /// List records matching `filters` (typically `source == X`) with their
    /// `content_hash`, for the per-source reconcile that decides what to
    /// re-embed.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or returns an error
    /// status.
    fn list_records(
        &self,
        store: &str,
        filters: Option<&Filter>,
    ) -> impl Future<Output = Result<Vec<StoredRecord>>> + Send;

    /// Upload one document under its `external_id`.
    ///
    /// # Errors
    /// Returns an error if the upload fails at either the file or attach step.
    fn upload(&self, store: &str, document: Document) -> impl Future<Output = Result<()>> + Send;

    /// Delete one record by `external_id`. Used by an explicit garbage
    /// collection pass, never by ordinary code sync.
    ///
    /// # Errors
    /// Returns an error if the delete request fails.
    fn delete(&self, store: &str, external_id: &str) -> impl Future<Output = Result<()>> + Send;

    /// Search one or more stores for a query, optionally constrained by
    /// metadata `filters`.
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
        filters: Option<&Filter>,
    ) -> impl Future<Output = Result<Vec<SearchHit>>> + Send;

    /// Grep one or more stores with a regular expression over the same chunks
    /// search covers, optionally constrained by metadata `filters`.
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
        filters: Option<&Filter>,
    ) -> impl Future<Output = Result<Vec<SearchHit>>> + Send;

    /// Ask a natural-language question against one or more stores, optionally
    /// constrained by metadata `filters`.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&Filter>,
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
/// Keyed by `external_id` like the real store, so the dedup behavior (a repeated
/// id is an overwrite) is exercised faithfully. It evaluates metadata filters
/// and reports each hit's [`Source`], so the source-aware projection and filter
/// wiring are testable with no network.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    files: HashMap<String, Stored>,
    upload_calls: usize,
}

#[derive(Debug)]
struct Stored {
    document: Document,
    source: Source,
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

    /// How many distinct records are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().files.len()
    }

    /// Whether the store holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().files.is_empty()
    }
}

fn source_of(document: &Document) -> Result<Source> {
    document
        .meta_json
        .get(search_meta::keys::SOURCE)
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<Source>().ok())
        .context(InvalidMetadataSnafu {
            external_id: document.external_id.clone(),
            key: search_meta::keys::SOURCE,
        })
}

impl Store for MemoryStore {
    async fn ensure_store(&self, _name: &str) -> Result<()> {
        Ok(())
    }

    async fn list_external_ids(&self, _store: &str) -> Result<HashSet<String>> {
        Ok(self.lock().files.keys().cloned().collect())
    }

    async fn list_records(
        &self,
        _store: &str,
        filters: Option<&Filter>,
    ) -> Result<Vec<StoredRecord>> {
        let inner = self.lock();
        let records = inner
            .files
            .values()
            .filter(|stored| filters.is_none_or(|f| matches_filter(&stored.document.meta_json, f)))
            .map(|stored| StoredRecord {
                external_id: stored.document.external_id.clone(),
                content_hash: Some(stored.document.content_hash.clone()),
            })
            .collect();
        drop(inner);
        Ok(records)
    }

    async fn upload(&self, _store: &str, document: Document) -> Result<()> {
        let source = source_of(&document)?;
        let mut inner = self.lock();
        inner.upload_calls += 1;
        inner
            .files
            .insert(document.external_id.clone(), Stored { document, source });
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
        filters: Option<&Filter>,
    ) -> Result<Vec<SearchHit>> {
        let needle = query.to_lowercase();
        Ok(self.scan(top_k, filters, |line| line.to_lowercase().contains(&needle)))
    }

    async fn grep(
        &self,
        _stores: &[String],
        pattern: &str,
        top_k: usize,
        options: GrepOptions,
        filters: Option<&Filter>,
    ) -> Result<Vec<SearchHit>> {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(!options.case_sensitive)
            .build()
            .with_context(|_| InvalidPatternSnafu {
                pattern: pattern.to_owned(),
            })?;
        Ok(self.scan(top_k, filters, |line| regex.is_match(line)))
    }

    async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&Filter>,
    ) -> Result<Answer> {
        let sources = self.search(stores, query, top_k, options, filters).await?;
        Ok(Answer {
            answer: "mock answer from MemoryStore".to_owned(),
            sources,
        })
    }

    async fn store_status(&self, _store: &str) -> Result<StoreStatus> {
        Ok(StoreStatus {
            pending: 0,
            in_progress: 0,
        })
    }
}

impl MemoryStore {
    /// Shared line scan for search and grep: match each stored record's body
    /// lines with `line_matches`, honoring the metadata `filters`, and report
    /// each hit with its source and content hash.
    fn scan(
        &self,
        top_k: usize,
        filters: Option<&Filter>,
        line_matches: impl Fn(&str) -> bool,
    ) -> Vec<SearchHit> {
        let inner = self.lock();
        let mut hits = Vec::new();
        for stored in inner.files.values() {
            if !filters.is_none_or(|f| matches_filter(&stored.document.meta_json, f)) {
                continue;
            }
            let text = String::from_utf8_lossy(&stored.document.body);
            for (index, line) in text.lines().enumerate() {
                if hits.len() >= top_k {
                    break;
                }
                if line_matches(line) {
                    hits.push(SearchHit {
                        source: stored.source,
                        hash: Some(stored.document.content_hash.clone()),
                        // Code records label by `path`; record sources by `title`.
                        path: stored
                            .document
                            .meta_json
                            .get(search_meta::keys::PATH)
                            .or_else(|| stored.document.meta_json.get(search_meta::keys::TITLE))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        text: line.to_owned(),
                        score: 1.0,
                        start_line: u32::try_from(index).ok(),
                        num_lines: Some(1),
                    });
                }
            }
            if hits.len() >= top_k {
                break;
            }
        }
        drop(inner);
        hits
    }
}

/// Evaluate a metadata filter against a flat metadata object. Covers the
/// operators the offline test double needs; comparison operators it does not
/// model evaluate to `false` so a test never silently passes on an unsupported
/// shape.
fn matches_filter(meta: &serde_json::Value, filter: &Filter) -> bool {
    match filter {
        Filter::Condition(condition) => matches_condition(meta, condition),
        Filter::Group(group) => {
            group.all.as_ref().is_none_or(|fs| fs.iter().all(|f| matches_filter(meta, f)))
                && group.any.as_ref().is_none_or(|fs| fs.iter().any(|f| matches_filter(meta, f)))
                && group
                    .none
                    .as_ref()
                    .is_none_or(|fs| !fs.iter().any(|f| matches_filter(meta, f)))
        }
    }
}

fn matches_condition(meta: &serde_json::Value, condition: &Condition) -> bool {
    let actual = meta.get(&condition.key);
    match condition.operator {
        Operator::Eq => actual == Some(&condition.value),
        Operator::NotEq => actual != Some(&condition.value),
        Operator::In => json_contains(&condition.value, actual),
        Operator::NotIn => !json_contains(&condition.value, actual),
        Operator::StartsWith => match (actual.and_then(serde_json::Value::as_str), condition.value.as_str()) {
            (Some(value), Some(prefix)) => value.starts_with(prefix),
            _ => false,
        },
        Operator::Like => match (actual.and_then(serde_json::Value::as_str), condition.value.as_str()) {
            (Some(value), Some(needle)) => value.contains(needle),
            _ => false,
        },
        _ => false,
    }
}

/// Whether `needle` (the actual metadata value) is an element of `haystack`
/// (the filter's array value), or equals it when `haystack` is a scalar.
fn json_contains(haystack: &serde_json::Value, needle: Option<&serde_json::Value>) -> bool {
    let Some(needle) = needle else { return false };
    haystack
        .as_array()
        .map_or_else(|| haystack == needle, |items| items.iter().any(|item| item == needle))
}
