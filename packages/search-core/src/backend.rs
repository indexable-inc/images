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
use snafu::{OptionExt as _, ResultExt as _};
use source_meta::{Document, Source};

use crate::error::{InvalidMetadataSnafu, InvalidPatternSnafu, Result};

/// Knobs forwarded to the backend's search call.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Second-stage reranker selection: a toggle or a pinned model
    /// (defaults to [`mixedbread::DEFAULT_RERANK_MODEL`] at the CLI/binding edge).
    pub rerank: mixedbread::Rerank,
    /// Let the backend plan and run several searches itself (a toggle or a
    /// tuned config). When enabled the backend ignores `rerank` and
    /// `rewrite_query` (the agent owns decomposition and ranking).
    pub agentic: mixedbread::Agentic,
    /// Rewrite the query backend-side before embedding it.
    pub rewrite_query: bool,
    /// Apply the store's backend-side search rules (the backend default).
    pub apply_search_rules: bool,
}

impl Default for SearchOptions {
    /// The interactive defaults: backend-default reranking on, everything
    /// else as the backend would behave with no options at all.
    fn default() -> Self {
        Self {
            rerank: mixedbread::Rerank::server_default(),
            agentic: mixedbread::Agentic::off(),
            rewrite_query: false,
            apply_search_rules: true,
        }
    }
}

/// Knobs forwarded to the backend's question-answering call: the retrieval
/// stage reuses [`SearchOptions`]; the answering stage adds its own toggles.
#[derive(Debug, Clone, Default)]
pub struct AskOptions {
    /// Search knobs for the retrieval stage (reranking, agentic, rules).
    pub search: SearchOptions,
    /// Whether the answer cites its sources with `<cite i="N"/>` markers.
    /// `None` applies the backend default (on); citations are what the
    /// projection rewrites onto the displayed source list.
    pub cite: Option<bool>,
    /// Whether the answer may draw on multimodal context. `None` applies the
    /// backend default (on); the corpus is text-only, so `Some(false)` is an
    /// explicit opt-out only.
    pub multimodal: Option<bool>,
    /// Extra instructions for the answering model.
    pub instructions: Option<String>,
}

impl From<SearchOptions> for AskOptions {
    /// Question-answering with custom retrieval knobs and the backend's
    /// default answering behavior — the pre-[`AskOptions`] call shape.
    fn from(search: SearchOptions) -> Self {
        Self {
            search,
            ..Self::default()
        }
    }
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
    /// Provenance read from the stored metadata, when present. These are what
    /// let a consumer judge staleness (timestamp) and pivot from a hit to its
    /// origin (session, repo, URL) instead of hitting a dead end.
    pub provenance: Provenance,
}

/// Identity and recency metadata carried by a hit, all optional because each
/// source writes a different subset (see `source-meta`'s `keys`).
#[derive(Debug, Clone, Default)]
pub struct Provenance {
    /// Epoch-second timestamp of the record (the primary recency axis).
    pub timestamp: Option<i64>,
    /// OS user that authored the record.
    pub user: Option<String>,
    /// Short hostname the record was recorded on.
    pub host: Option<String>,
    /// Session id (Claude Code transcript, codex, or shell session).
    pub session_id: Option<String>,
    /// The record's caller-assigned external id (e.g. `claude:{session}:{uuid}`).
    pub external_id: Option<String>,
    /// Canonical web URL (GitHub items, Linear issues, web hits).
    pub url: Option<String>,
    /// Repository slug for code and git-commit records.
    pub repo: Option<String>,
    /// Project slug (the working directory a transcript was recorded under).
    pub project: Option<String>,
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
    /// The `source` stored in its metadata, when present. A caller that listed
    /// with a `source == X` filter can cross-check it: a record claiming
    /// another source (or none) means the backend did not apply the scope, and
    /// anything derived from the listing — deletes above all — must not run.
    pub source: Option<String>,
    /// The backend's own id for this file object, when it has one. A retried
    /// upload can leave several file objects under one `external_id`, and only
    /// this id can address one of them without ambiguity (deletes accept it in
    /// place of the external id). `None` for backends keyed purely by
    /// external id (the in-memory test store).
    pub file_id: Option<String>,
    /// When the backend created this file object (RFC 3339, UTC), when
    /// reported. RFC 3339 in one zone orders lexicographically; the GC pass
    /// compares these as strings to keep the newest among exact duplicates.
    pub created_at: Option<String>,
}

/// A vector store that holds documents and answers searches.
pub trait Store {
    /// Ensure the named store exists, creating it if absent.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or the store cannot be
    /// created.
    fn ensure_store(&self, name: &str) -> impl Future<Output = Result<()>> + Send;

    /// List the `external_id`s already present in the store, optionally scoped
    /// by `filters`. Used by the code sync path, where the id is the content
    /// hash; passing a `source == code AND repo == <slug>` filter keeps the
    /// listing proportional to the repo being synced rather than the whole
    /// shared store.
    ///
    /// # Errors
    /// Returns an error if the backend cannot be reached or returns an error
    /// status.
    fn list_external_ids(
        &self,
        store: &str,
        filters: Option<&Filter>,
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

    /// List chunks purely by metadata `filters` — no semantic scoring — sorted
    /// by `sort_by` (e.g. descending `timestamp` for a newest-first feed).
    /// `top_k` caps the result; the endpoint has no cursor.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn list_chunks(
        &self,
        stores: &[String],
        top_k: usize,
        filters: Option<&Filter>,
        sort_by: Option<&mixedbread::SortBy>,
    ) -> impl Future<Output = Result<Vec<SearchHit>>> + Send;

    /// Count distinct metadata values per key across one or more stores,
    /// optionally scoped by `filters`: `facets(.., &["source"], ..)` answers
    /// "how many documents per source" in one call. Counts are per document
    /// (store file), not per chunk. This is the census primitive behind
    /// `search stats`.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be
    /// decoded.
    fn facets(
        &self,
        stores: &[String],
        keys: &[&str],
        filters: Option<&Filter>,
    ) -> impl Future<Output = Result<mixedbread::Facets>> + Send;

    /// Ask a natural-language question against one or more stores, optionally
    /// constrained by metadata `filters`. `options` carries both the retrieval
    /// knobs and the answering-stage toggles (citations, multimodal context,
    /// instructions).
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: AskOptions,
        filters: Option<&Filter>,
    ) -> impl Future<Output = Result<Answer>> + Send;

    /// Fetch indexing progress for a store, used to wait until newly uploaded
    /// files are embedded and searchable.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn store_status(&self, store: &str) -> impl Future<Output = Result<StoreStatus>> + Send;

    /// Fetch one file's indexing status by `external_id`, so a caller can wait
    /// on exactly the files it uploaded instead of gating on the store-wide
    /// counts [`Store::store_status`] reports (the store is shared across
    /// sources and writers, so an unrelated backlog must not block a wait).
    /// `Ok(None)` when the store holds no such file.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response cannot be decoded.
    fn file_status(
        &self,
        store: &str,
        external_id: &str,
    ) -> impl Future<Output = Result<Option<mixedbread::FileStatus>>> + Send;
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
        .get(source_meta::keys::SOURCE)
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<Source>().ok())
        .context(InvalidMetadataSnafu {
            external_id: document.external_id.clone(),
            key: source_meta::keys::SOURCE,
        })
}

impl Store for MemoryStore {
    async fn ensure_store(&self, _name: &str) -> Result<()> {
        Ok(())
    }

    async fn list_external_ids(
        &self,
        _store: &str,
        filters: Option<&Filter>,
    ) -> Result<HashSet<String>> {
        let inner = self.lock();
        Ok(inner
            .files
            .values()
            .filter(|stored| filters.is_none_or(|f| matches_filter(&stored.document.meta_json, f)))
            .map(|stored| stored.document.external_id.clone())
            .collect())
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
                source: Some(stored.source.as_str().to_owned()),
                // Keyed purely by external id: one record per id, no separate
                // file-object identity and no creation timestamp.
                file_id: None,
                created_at: None,
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

    async fn list_chunks(
        &self,
        _stores: &[String],
        top_k: usize,
        filters: Option<&Filter>,
        sort_by: Option<&mixedbread::SortBy>,
    ) -> Result<Vec<SearchHit>> {
        let inner = self.lock();
        let mut hits: Vec<SearchHit> = inner
            .files
            .values()
            .filter(|stored| filters.is_none_or(|f| matches_filter(&stored.document.meta_json, f)))
            .map(|stored| {
                let meta = &stored.document.meta_json;
                SearchHit {
                    source: stored.source.clone(),
                    hash: Some(stored.document.content_hash.clone()),
                    path: meta
                        .get(source_meta::keys::PATH)
                        .or_else(|| meta.get(source_meta::keys::TITLE))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    text: String::from_utf8_lossy(&stored.document.body).into_owned(),
                    // No semantic scoring on this path; mirror the API's
                    // placeholder.
                    score: 1.0,
                    start_line: None,
                    num_lines: None,
                    provenance: provenance_of(Some(meta)),
                }
            })
            .collect();
        drop(inner);
        if let Some(sort) = sort_by {
            // The double models numeric metadata sorts (timestamps); a missing
            // key sorts last in either direction, like SQL NULLS LAST.
            hits.sort_by(|a, b| {
                let key = |hit: &SearchHit| match sort.field.as_str() {
                    "timestamp" => hit.provenance.timestamp,
                    _ => None,
                };
                let (ka, kb) = (key(a), key(b));
                match (ka, kb) {
                    (Some(a), Some(b)) if sort.ascending => a.cmp(&b),
                    (Some(a), Some(b)) => b.cmp(&a),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
        }
        hits.truncate(top_k);
        Ok(hits)
    }

    async fn facets(
        &self,
        _stores: &[String],
        keys: &[&str],
        filters: Option<&Filter>,
    ) -> Result<mixedbread::Facets> {
        let inner = self.lock();
        let mut facets = mixedbread::Facets::new();
        for stored in inner
            .files
            .values()
            .filter(|stored| filters.is_none_or(|f| matches_filter(&stored.document.meta_json, f)))
        {
            for key in keys {
                let Some(value) = stored.document.meta_json.get(*key) else {
                    continue;
                };
                // The server stringifies values (JSON object keys are
                // strings); mirror that, without quoting real strings.
                let value = match value {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                };
                *facets
                    .entry((*key).to_owned())
                    .or_default()
                    .entry(value)
                    .or_insert(0) += 1;
            }
        }
        drop(inner);
        Ok(facets)
    }

    async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: AskOptions,
        filters: Option<&Filter>,
    ) -> Result<Answer> {
        use std::fmt::Write as _;

        let sources = self
            .search(stores, query, top_k, options.search, filters)
            .await?;
        // Cite every source in raw order, the way the production backend
        // emits `<cite i="N"/>` markers indexing its own source list, so the
        // citation-remapping projection is exercised offline. An explicit
        // cite opt-out yields marker-free prose, like the real backend.
        let mut answer = "mock answer from MemoryStore ".to_owned();
        if options.cite != Some(false) {
            for index in 0..sources.len() {
                write!(answer, "<cite i=\"{index}\"/>").expect("writing to a String cannot fail");
            }
        }
        Ok(Answer { answer, sources })
    }

    async fn store_status(&self, _store: &str) -> Result<StoreStatus> {
        Ok(StoreStatus {
            pending: 0,
            in_progress: 0,
        })
    }

    async fn file_status(
        &self,
        _store: &str,
        external_id: &str,
    ) -> Result<Option<mixedbread::FileStatus>> {
        // MemoryStore embeds synchronously: a stored file is already searchable.
        Ok(self
            .lock()
            .files
            .contains_key(external_id)
            .then_some(mixedbread::FileStatus::Completed))
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
                        source: stored.source.clone(),
                        hash: Some(stored.document.content_hash.clone()),
                        // Code records label by `path`; record sources by `title`.
                        path: stored
                            .document
                            .meta_json
                            .get(source_meta::keys::PATH)
                            .or_else(|| stored.document.meta_json.get(source_meta::keys::TITLE))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned),
                        text: line.to_owned(),
                        score: 1.0,
                        start_line: u32::try_from(index).ok(),
                        num_lines: Some(1),
                        provenance: provenance_of(Some(&stored.document.meta_json)),
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

/// Read the provenance fields out of a record's flat metadata object.
///
/// Shared by the production adapter and the test double so a hit's identity
/// fields are extracted in exactly one place.
// `pub(crate)`: only the adapter and this module's double need it; the lint
// fires because the module is private, but the function is re-shared via the
// crate, not the public API.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn provenance_of(metadata: Option<&serde_json::Value>) -> Provenance {
    let get_str = |key: &str| {
        metadata
            .and_then(|m| m.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    Provenance {
        timestamp: metadata
            .and_then(|m| m.get(source_meta::keys::TIMESTAMP))
            .and_then(serde_json::Value::as_i64),
        user: get_str(source_meta::keys::USER),
        host: get_str(source_meta::keys::HOST),
        session_id: get_str(source_meta::keys::SESSION_ID),
        external_id: get_str(source_meta::keys::EXTERNAL_ID),
        url: get_str(source_meta::keys::URL),
        repo: get_str(source_meta::keys::REPO),
        project: get_str(source_meta::keys::PROJECT),
    }
}

/// Evaluate a metadata filter against a flat metadata object. Covers the
/// operators the offline test double needs (including numeric `gt`/`gte`/
/// `lt`/`lte`, which model the server's timestamp range filters); operators it
/// does not model evaluate to `false` so a test never silently passes on an
/// unsupported shape.
fn matches_filter(meta: &serde_json::Value, filter: &Filter) -> bool {
    match filter {
        Filter::Condition(condition) => matches_condition(meta, condition),
        Filter::Group(group) => {
            group
                .all
                .as_ref()
                .is_none_or(|fs| fs.iter().all(|f| matches_filter(meta, f)))
                && group
                    .any
                    .as_ref()
                    .is_none_or(|fs| fs.iter().any(|f| matches_filter(meta, f)))
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
        Operator::StartsWith => match (
            actual.and_then(serde_json::Value::as_str),
            condition.value.as_str(),
        ) {
            (Some(value), Some(prefix)) => value.starts_with(prefix),
            _ => false,
        },
        Operator::Like => match (
            actual.and_then(serde_json::Value::as_str),
            condition.value.as_str(),
        ) {
            (Some(value), Some(needle)) => value.contains(needle),
            _ => false,
        },
        Operator::Gt | Operator::Gte | Operator::Lt | Operator::Lte => {
            // Numeric comparison only (the production use is epoch-second
            // timestamps); a non-numeric side evaluates to false.
            match (
                actual.and_then(serde_json::Value::as_f64),
                condition.value.as_f64(),
            ) {
                (Some(value), Some(bound)) => match condition.operator {
                    Operator::Gt => value > bound,
                    Operator::Gte => value >= bound,
                    Operator::Lt => value < bound,
                    _ => value <= bound,
                },
                _ => false,
            }
        }
        _ => false,
    }
}

/// Whether `needle` (the actual metadata value) is an element of `haystack`
/// (the filter's array value), or equals it when `haystack` is a scalar.
fn json_contains(haystack: &serde_json::Value, needle: Option<&serde_json::Value>) -> bool {
    let Some(needle) = needle else { return false };
    haystack.as_array().map_or_else(
        || haystack == needle,
        |items| items.iter().any(|item| item == needle),
    )
}
