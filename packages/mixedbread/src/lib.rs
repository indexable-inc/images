//! Minimal async client for the [Mixedbread](https://www.mixedbread.com) vector
//! store API. It owns HTTP and JSON shapes only; it carries no domain logic, so
//! it can back a search tool or any other consumer.
//!
//! Endpoints covered: store create/get (`/v1/stores`), the two-step file upload
//! (`/v1/files` then `/v1/stores/{store}/files`), file listing, per-file status,
//! and deletion, search (`/v1/stores/search`), regex grep (`/v1/stores/grep`),
//! metadata-only chunk listing (`/v1/stores/list-chunks`),
//! question-answering (`/v1/stores/question-answering`), query
//! enhancement (`/v1/stores/queries/enhance`), metadata facets
//! (`/v1/stores/metadata-facets`), and the per-store event histogram
//! (`/v1/stores/{store}/events/histogram`).

use std::path::PathBuf;
use std::time::Duration;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::{Client as HttpClient, StatusCode};
use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu};

pub mod auth;
pub mod enhance;
pub mod filter;

pub use enhance::{EnhancedQuery, FilterMode, SortDirection};
pub use filter::{Condition, Filter, Group, Operator};

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.mixedbread.com";

/// Page size for paginated `files/list` requests. The API rejects anything
/// over 100 (HTTP 422), so this is the ceiling; listing follows a cursor and is
/// therefore inherently sequential. Callers that must reconcile against a large
/// store should avoid listing when local state already says nothing changed,
/// rather than expecting a bigger page.
const LIST_PAGE_SIZE: u32 = 100;

/// Environment variable holding the API key.
pub const API_KEY_ENV: &str = "MXBAI_API_KEY";

/// Bytes percent-encoded when an external id is spliced into a URL path: the
/// url crate's path-segment set (controls, space, and the URL delimiters) plus
/// `/` and `%`. External ids may contain `/` (the API supports path-shaped ids
/// like `github:org/repo`); unencoded, such an id splits the route and the
/// API answers 404 for a file that exists.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'#')
    .add(b'?')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'%');

/// Retries for a request that returns a retryable status (`429 Too Many
/// Requests` or any `5xx`). The fleet runs one indexer per host against a single
/// shared Mixedbread key with 16 uploads in flight each, so transient 429s are
/// expected under load; we honor the server's `Retry-After` and otherwise back
/// off with jitter rather than failing the source. Mixedbread's own error
/// guidance is to retry these with exponential backoff:
/// <https://mixedbread.com/api-reference/error-handling>.
const MAX_RETRIES: u32 = 6;

/// Base backoff: attempt `n` sleeps within `[base*2^n / 2, base*2^n]` (equal
/// jitter), capped at [`BACKOFF_CAP`], unless the server asks for longer.
const BACKOFF_BASE: Duration = Duration::from_millis(500);

/// Upper bound on a single backoff sleep, and on a server `Retry-After` we will
/// honor, so one absurd header value cannot stall a whole sync.
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// Bound on establishing a connection. reqwest's default is none, so one
/// unanswered SYN under load would hold a whole sync hostage.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound on one whole request, from send through reading the response body.
/// reqwest's default is no timeout: when Mixedbread sheds a stream under load
/// the TCP connection stays ESTAB on keepalives and the response never comes,
/// so the await never resolves and [`Client::send_retrying`]'s transport-retry
/// arm never runs. This wedged the ix leader's corpus-view reconcile bootstrap
/// mid-listing for over an hour (2026-06-10). 120s clears the slowest
/// legitimate requests (a 1 MiB multipart upload, a reranked search under
/// load); past that the stream is dead and the retry ladder takes over.
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

/// The crate's HTTP client builder, with every request bounded by
/// [`CONNECT_TIMEOUT`] and [`REQUEST_TIMEOUT`] so a shed stream surfaces as a
/// retryable transport error instead of an unbounded await. Every client in
/// this crate (store API here, token exchange in [`auth`]) builds from this.
pub(crate) fn bounded_http_builder() -> reqwest::ClientBuilder {
    HttpClient::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
}

/// Failures from the Mixedbread client.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The HTTP client could not be constructed.
    #[snafu(display("failed to build HTTP client: {source}"))]
    BuildClient {
        /// Underlying reqwest error.
        source: reqwest::Error,
    },

    /// The API key environment variable is unset or empty.
    #[snafu(display("{API_KEY_ENV} is not set; export a key from https://mixedbread.com"))]
    MissingApiKey,

    /// A request failed to send, or its body failed to decode.
    #[snafu(display("Mixedbread request failed: {source}"))]
    Http {
        /// Underlying reqwest error.
        source: reqwest::Error,
    },

    /// The API returned a non-success status.
    #[snafu(display("Mixedbread API returned {status}: {body}"))]
    Api {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },

    /// No credential was found in the environment or from `mgrep login`.
    #[snafu(display("no Mixedbread credential found; set {API_KEY_ENV} or run `mgrep login`"))]
    NoCredential,

    /// The `mgrep login` token file exists but could not be read.
    #[snafu(display("failed to read mgrep credential {}: {source}", path.display()))]
    ReadCredential {
        /// Credential file path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The `mgrep login` token file could not be parsed.
    #[snafu(display("failed to parse mgrep credential {}: {source}", path.display()))]
    CredentialParse {
        /// Credential file path.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// Exchanging the stored OAuth token for an API JWT failed.
    #[snafu(display(
        "failed to exchange mgrep credential for an API token ({status}: {body}); run `mgrep login`"
    ))]
    TokenExchange {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },

    /// The platform returned an empty token during exchange.
    #[snafu(display("platform returned no API token; run `mgrep login` again"))]
    EmptyJwt,

    /// The query-enhance endpoint answered success but carried no items. The
    /// schema promises exactly one, so an empty list is a server contract
    /// violation, not a "no filters extracted" result (that arrives as a query
    /// item with empty filters).
    #[snafu(display("queries/enhance returned no items"))]
    EnhanceEmpty,
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Default second-stage reranking model: Mixedbread's listwise reranker.
///
/// It reads the candidate set as a whole and lifts ranking quality across every
/// benchmark relative to the prior pointwise default.
/// See <https://www.mixedbread.com/blog/mxbai-rerank-v3-listwise>.
pub const DEFAULT_RERANK_MODEL: &str = "mixedbread-ai/mxbai-rerank-v3-listwise";

/// Second-stage reranker selection.
///
/// Serialized as the API's `rerank` field, which is `boolean | object`:
/// [`Rerank::Toggle`] serializes to a bare bool (so the legacy "just turn it
/// on/off" wire body is byte-for-byte unchanged), while [`Rerank::Model`]
/// serializes to the `RerankConfig` object form (`{ "model": "...", "top_k":
/// N }`, optional fields omitted) to pin a model and cap the reranked list.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum Rerank {
    /// Toggle the server's default reranker: `false` disables reranking,
    /// `true` applies the API-chosen default model.
    Toggle(bool),
    /// Apply a named reranking model, e.g. [`DEFAULT_RERANK_MODEL`].
    Model {
        /// Reranking model name forwarded to the API.
        model: String,
        /// Cap the result list after reranking. `None` keeps every reranked
        /// hit (the API default), so the legacy `{ "model": ... }` wire body
        /// is unchanged when unset.
        #[serde(skip_serializing_if = "Option::is_none")]
        top_k: Option<usize>,
    },
}

impl Rerank {
    /// Reranking disabled (wire `false`).
    #[must_use]
    pub const fn off() -> Self {
        Self::Toggle(false)
    }

    /// The API's default reranker (wire `true`).
    #[must_use]
    pub const fn server_default() -> Self {
        Self::Toggle(true)
    }

    /// Pin a specific reranking model.
    #[must_use]
    pub fn model(name: impl Into<String>) -> Self {
        Self::Model {
            model: name.into(),
            top_k: None,
        }
    }

    /// The listwise reranker ([`DEFAULT_RERANK_MODEL`]).
    #[must_use]
    pub fn listwise() -> Self {
        Self::model(DEFAULT_RERANK_MODEL)
    }
}

/// Agentic search selection.
///
/// Serialized as the API's `agentic` field, which is `boolean | object`:
/// [`Agentic::Toggle`] serializes to a bare bool (the legacy wire body),
/// while [`Agentic::Config`] serializes to the `AgenticSearchConfig` object
/// form to tune the agent. When agentic search is enabled the server ignores
/// `rewrite_query` and `rerank` (the agent owns decomposition and ranking).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum Agentic {
    /// Toggle agentic search with the server's default configuration.
    Toggle(bool),
    /// Agentic search with explicit tuning.
    Config(AgenticConfig),
}

impl Agentic {
    /// Agentic search disabled (wire `false`).
    #[must_use]
    pub const fn off() -> Self {
        Self::Toggle(false)
    }

    /// Agentic search with the server's defaults (wire `true`).
    #[must_use]
    pub const fn on() -> Self {
        Self::Toggle(true)
    }

    /// Whether agentic search is enabled in any form. A config object always
    /// enables it; only the bare `false` toggle disables it.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Toggle(false))
    }
}

/// Tuning for agentic multi-query search (the API's `AgenticSearchConfig`).
///
/// Every field is optional and omitted from the wire when unset, so the
/// server default applies. `media_content` and `verbose` are deliberately not
/// modeled: the corpus holds no image chunks, and `verbose` is documented by
/// the API schema as internal to the Mixedbread playground.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AgenticConfig {
    /// Maximum number of search rounds (API default 3, max 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
    /// Maximum queries per round (API default 4, max 10).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queries_per_round: Option<u32>,
    /// Require exactly `top_k` ranked chunks in the final list. Off by
    /// default: the agent gates results on its own judged relevance and may
    /// return fewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict_top_k: Option<bool>,
    /// Extra instructions for the search agent (followed only when not in
    /// conflict with the server's own rules; capped at 5000 chars by the API).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Sort order for [`Client::list_chunks`]: a metadata field path and direction.
///
/// Serializes to the API's `[field_path, ascending]` tuple form. An unprefixed
/// dot path targets file metadata (e.g. `timestamp`); `generated_metadata.*`
/// targets chunk metadata.
#[derive(Debug, Clone)]
pub struct SortBy {
    /// Metadata field path to sort on.
    pub field: String,
    /// Ascending (`true`) or descending (`false`).
    pub ascending: bool,
}

impl SortBy {
    /// Sort ascending on `field`.
    #[must_use]
    pub fn asc(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            ascending: true,
        }
    }

    /// Sort descending on `field` (e.g. newest-first on a timestamp).
    #[must_use]
    pub fn desc(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            ascending: false,
        }
    }
}

impl serde::Serialize for SortBy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The API accepts `string | [field_path, ascending]`; always emit the
        // tuple so the direction is explicit on the wire.
        (&self.field, self.ascending).serialize(serializer)
    }
}

/// Search tuning forwarded to the API (the `search_options` body field).
///
/// Every optional field is skipped when unset, so a caller that only sets
/// `rerank`/`agentic` produces the same wire body as before.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchOptions {
    /// Apply the second-stage reranker (toggle or a pinned model). Ignored by
    /// the server when agentic search is enabled.
    pub rerank: Rerank,
    /// Let the API plan and run multiple searches (toggle or tuned config).
    pub agentic: Agentic,
    /// Drop hits scoring below this threshold (`0.0..=1.0`). Used to keep a
    /// low-relevance source from crowding a multi-source result list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
    /// Ask the API to return each chunk's file metadata, so a result can be
    /// mapped back to its source. Skipped when `None` (API default applies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_metadata: Option<bool>,
    /// Rewrite the query server-side before embedding it. Skipped when `None`
    /// (API default `false`); ignored by the server when agentic search is
    /// enabled (the agent owns query decomposition).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite_query: Option<bool>,
    /// Apply the store's server-side search rules. Skipped when `None` (API
    /// default `true`); `Some(false)` bypasses the rules for one query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_search_rules: Option<bool>,
}

/// Question-answering tuning forwarded to the API alongside the search
/// options: the `qa_options` body object plus the request-level
/// `instructions` field.
///
/// Every field is optional and omitted from the wire when unset, so a caller
/// passing `QaOptions::default()` produces the same wire body as before the
/// type existed. The endpoint also accepts `stream: true` for a streamed
/// answer; the live API answers `500 internal_error` to it in every
/// combination tried (verified 2026-06-12, four attempts, with and without
/// `qa_options` and an SSE `Accept` header), so this client does not expose
/// streaming until the server-side feature works.
#[derive(Debug, Clone, Default)]
pub struct QaOptions {
    /// Whether the answer cites its sources with `<cite i="N"/>` markers
    /// indexing the returned source list. `None` applies the API default
    /// (`true`).
    pub cite: Option<bool>,
    /// Whether the answer may draw on multimodal (image/audio/video) context.
    /// `None` applies the API default (`true`); the shared corpus is text-only,
    /// so `Some(false)` only matters as an explicit opt-out.
    pub multimodal: Option<bool>,
    /// Extra instructions for the answering model (followed only when not in
    /// conflict with existing rules; capped at 8000 chars by the API).
    pub instructions: Option<String>,
}

/// File-id scoping for search and question-answering.
///
/// Restricts matching chunks to (or excludes) a set of store file UUIDs,
/// `AND`ed with any metadata `filters`. Serialized as the API's request-level
/// `file_ids` field, which is `[id, ...] | [operator, [id, ...]]`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum FileIds {
    /// Bare inclusion list (wire `["id", ...]`).
    Include(Vec<String>),
    /// Operator form (wire `["in" | "not_in", ["id", ...]]`). Only
    /// [`Operator::In`] and [`Operator::NotIn`] are meaningful here; the API
    /// rejects other operators.
    Scoped(Operator, Vec<String>),
}

impl FileIds {
    /// Keep only chunks from these store files.
    #[must_use]
    pub const fn include(ids: Vec<String>) -> Self {
        Self::Include(ids)
    }

    /// Exclude chunks from these store files.
    #[must_use]
    pub const fn exclude(ids: Vec<String>) -> Self {
        Self::Scoped(Operator::NotIn, ids)
    }
}

/// A file as reported by the store's file listing.
#[derive(Debug, Clone)]
pub struct StoredFile {
    /// The store file object's own id. Unlike `external_id` it is unique per
    /// file object, so it is the only unambiguous delete handle when a retried
    /// upload has left several file objects under one external id.
    pub id: Option<String>,
    /// Caller-assigned external id, if any.
    pub external_id: Option<String>,
    /// Arbitrary metadata attached at upload time.
    pub metadata: Option<serde_json::Value>,
    /// Creation timestamp (RFC 3339, UTC) as reported by the API. RFC 3339 in
    /// one zone orders lexicographically, so callers compare these as strings
    /// to find the newest among duplicates.
    pub created_at: Option<String>,
}

/// One scored chunk returned by search or question-answering.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Matched snippet text.
    pub text: Option<String>,
    /// Relevance score.
    pub score: f32,
    /// Filename or URL of the source.
    pub filename: Option<String>,
    /// One-based start line within the source file, as reported by the API.
    pub start_line: Option<u32>,
    /// Line span of the chunk (`end_line - start_line`), as reported by the API,
    /// so a chunk of N lines reports `num_lines == N - 1`. Consumers that want a
    /// line count add one.
    pub num_lines: Option<u32>,
    /// Metadata attached to the source file at upload time.
    pub metadata: Option<serde_json::Value>,
}

/// A question-answering response.
#[derive(Debug, Clone)]
pub struct AnswerResponse {
    /// Synthesized answer text.
    pub answer: String,
    /// Chunks the answer drew from.
    pub sources: Vec<Chunk>,
}

/// One reranked document from [`Client::rerank`]: its position in the submitted
/// input list and its relevance score, already sorted most-relevant first by
/// the API.
#[derive(Debug, Clone, Copy)]
pub struct RerankHit {
    /// Index into the `input` slice the caller submitted.
    pub index: usize,
    /// Relevance score for the query.
    pub score: f32,
}

/// Distinct values and their file counts, per metadata key.
///
/// Returned by [`Client::metadata_facets`]: `facets["source"]["shell"]` is the
/// number of store files whose `source` metadata equals `shell`. Keys are
/// stringified by the server, so numeric metadata values arrive as digit
/// strings.
pub type Facets = std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>;

/// Hard ceiling on [`FacetLimits::max_files`] (the API rejects a larger scan
/// bound with HTTP 422).
///
/// A store holding more files than the scan bound reports truncated counts —
/// verified live on a >100k-file store, whose per-source counts summed to
/// exactly this cap (2026-06-12).
pub const FACETS_MAX_FILES: u32 = 100_000;

/// Caps for a metadata-facet computation. Every field is optional and omitted
/// from the wire when unset, so the API defaults apply.
#[derive(Debug, Clone, Copy, Default)]
pub struct FacetLimits {
    /// Maximum number of distinct metadata fields (keys) returned (API
    /// default 64, max 256). Irrelevant when the request names its facets.
    pub max_fields: Option<u32>,
    /// Maximum number of distinct values returned per field, ranked by count
    /// (API default 32, max 256).
    pub max_values_per_field: Option<u32>,
    /// Maximum number of store files scanned to compute the counts (API
    /// default 10 000, max [`FACETS_MAX_FILES`]).
    pub max_files: Option<u32>,
}

/// Event families accepted by the store telemetry endpoints' `event_types`
/// filter ([`Client::events_histogram`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// File ingestion (upload through embedding).
    Ingestion,
    /// Semantic search requests.
    Search,
    /// Agentic (multi-round) search requests.
    AgenticSearch,
    /// Regex grep requests.
    Grep,
}

/// One event type's count within one time bucket, as returned by
/// [`Client::events_histogram`].
#[derive(Debug, Clone, Deserialize)]
pub struct HistogramBucket {
    /// Bucket start, RFC 3339.
    pub bucket_start: String,
    /// Bucket end, RFC 3339.
    pub bucket_end: String,
    /// Dotted event name (e.g. `store.file.completed`, `store.search`). An
    /// open set server-side, so it stays a string rather than an enum.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Number of events of this type in the bucket.
    pub event_count: u64,
}

/// Indexing progress for a store: how many files are still being processed.
#[derive(Debug, Clone, Copy)]
pub struct StoreStatus {
    /// Files queued but not yet processed.
    pub pending: u64,
    /// Files currently being embedded.
    pub in_progress: u64,
}

/// One store file's indexing status: the `status` field of the store-file
/// object (`GET /v1/stores/{store}/files/{id}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    /// Queued but not yet processed.
    Pending,
    /// Currently being parsed and embedded.
    InProgress,
    /// Embedded and searchable.
    Completed,
    /// Processing failed; the store retries on its own schedule, not the
    /// caller's.
    Failed,
    /// Processing was cancelled.
    Cancelled,
    /// A status string this client does not know. [`FileStatus::is_settled`]
    /// treats it as settled so a new server-side state can never wedge a
    /// caller's wait loop; the wait's own timeout still bounds the worst case.
    #[serde(other)]
    Unknown,
}

impl FileStatus {
    /// Whether the store has stopped working on this file (embedded, failed,
    /// cancelled, or unrecognized), i.e. polling longer cannot change anything.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        !matches!(self, Self::Pending | Self::InProgress)
    }
}

/// Async client bound to a base URL and API key.
#[derive(Debug, Clone)]
pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: String,
}

impl Client {
    /// Build a client for an explicit base URL and API key.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Result<Self> {
        let http = bounded_http_builder().build().context(BuildClientSnafu)?;
        Ok(Self {
            http,
            base_url: base_url.into(),
            api_key: api_key.into(),
        })
    }

    /// Build a client reading the API key from [`API_KEY_ENV`].
    ///
    /// # Errors
    /// Returns an error if the key is unset or empty, or the client cannot be
    /// built.
    pub fn from_env(base_url: impl Into<String>) -> Result<Self> {
        let api_key = std::env::var(API_KEY_ENV)
            .ok()
            .filter(|value| !value.is_empty())
            .context(MissingApiKeySnafu)?;
        Self::new(base_url, api_key)
    }

    /// Build a client resolving the credential the way a user expects: the
    /// `MXBAI_API_KEY` environment variable if set, otherwise the token stored
    /// by `mgrep login` (exchanged for an API JWT). See [`auth::resolve_token`].
    ///
    /// # Errors
    /// Returns an error if no credential can be resolved, the token cannot be
    /// exchanged, or the client cannot be built.
    pub async fn from_login(base_url: impl Into<String>) -> Result<Self> {
        let api_key = auth::resolve_token(auth::PLATFORM_URL).await?;
        Self::new(base_url, api_key)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Send a request with bearer auth, retrying on `429`/`5xx` (with
    /// `Retry-After`-aware, jittered backoff) and on transport-level send
    /// failures (connection reset, HTTP/2 error, timeout) where no response
    /// arrived. All failures share one [`MAX_RETRIES`] budget per request.
    ///
    /// `build` must produce a fresh [`reqwest::RequestBuilder`] on each call: a
    /// request body (notably multipart) is consumed by `send`, so a retry needs
    /// a new one. It is fallible so a body that can fail to assemble (a
    /// multipart `Part` rejecting its MIME) surfaces as an error rather than a
    /// panic; that error is deterministic, so it is returned on the first call
    /// without retrying. A non-retryable response (including the final attempt)
    /// is returned as-is for the caller to decode or turn into an [`Error::Api`].
    async fn send_retrying(
        &self,
        build: impl Fn() -> Result<reqwest::RequestBuilder>,
    ) -> Result<reqwest::Response> {
        let mut attempt: u32 = 0;
        loop {
            let wait = match build()?.bearer_auth(&self.api_key).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let retryable =
                        status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
                    if !retryable || attempt >= MAX_RETRIES {
                        return Ok(resp);
                    }
                    // Honor a server `Retry-After`, but never wait less than our
                    // jittered backoff: a `Retry-After: 0`/`1` under load would
                    // otherwise make every concurrent upload retry in lockstep,
                    // the very thundering herd this exists to avoid.
                    retry_after(&resp)
                        .map_or_else(|| backoff(attempt), |hint| hint.max(backoff(attempt)))
                }
                // A transport-level failure (connection reset, HTTP/2 error,
                // timeout): the send errored before a usable response, so we
                // retry. Mixedbread sheds connections under load, so these are
                // common during a large sync and were previously fatal per
                // source. Note the send *may* have reached the server (it can
                // error after the body was transmitted), so retrying the
                // non-idempotent `POST /v1/files` can orphan a file object —
                // the same caveat as the 5xx path, documented at the call site.
                Err(err) => {
                    if attempt >= MAX_RETRIES {
                        return Err(err).context(HttpSnafu);
                    }
                    backoff(attempt)
                }
            };
            attempt += 1;
            tokio::time::sleep(wait).await;
        }
    }

    /// Ensure the named store exists, creating it if absent.
    ///
    /// # Errors
    /// Returns an error if the store cannot be fetched or created.
    pub async fn ensure_store(&self, name: &str) -> Result<()> {
        let get_url = self.url(&format!("/v1/stores/{name}"));
        let resp = self
            .send_retrying(|| Ok(self.http.get(get_url.as_str())))
            .await?;
        if resp.status().is_success() {
            return Ok(());
        }
        if resp.status() != StatusCode::NOT_FOUND {
            return Err(api_error(resp).await);
        }
        let create_url = self.url("/v1/stores");
        let created = self
            .send_retrying(|| {
                Ok(self
                    .http
                    .post(create_url.as_str())
                    .json(&serde_json::json!({ "name": name })))
            })
            .await?;
        expect_ok(created).await
    }

    /// List files in a store, following cursor pagination.
    ///
    /// # Errors
    /// Returns an error if any page request fails or cannot be decoded.
    pub async fn list_files(
        &self,
        store: &str,
        filters: Option<&filter::Filter>,
    ) -> Result<Vec<StoredFile>> {
        let mut files = Vec::new();
        let mut after: Option<String> = None;
        let list_url = self.url(&format!("/v1/stores/{store}/files/list"));
        loop {
            let request = ListRequest {
                limit: LIST_PAGE_SIZE,
                after: after.as_deref(),
                filters,
            };
            let resp = self
                .send_retrying(|| Ok(self.http.post(list_url.as_str()).json(&request)))
                .await?;
            let page: ListResponse = decode(resp).await?;
            for item in page.data {
                files.push(StoredFile {
                    id: item.id,
                    external_id: item.external_id,
                    metadata: item.metadata,
                    created_at: item.created_at,
                });
            }
            match page.pagination {
                Some(Pagination {
                    has_more: true,
                    last_cursor: Some(cursor),
                }) => after = Some(cursor),
                _ => break,
            }
        }
        Ok(files)
    }

    /// Upload one file: send the bytes to `/v1/files`, then attach the returned
    /// id to the store under `external_id` with `metadata`.
    ///
    /// # Errors
    /// Returns an error if either request fails.
    pub async fn upload_file(
        &self,
        store: &str,
        content: Vec<u8>,
        file_name: &str,
        external_id: &str,
        mime: &str,
        metadata: serde_json::Value,
    ) -> Result<()> {
        // A multipart `Part` is not `Clone`, so the retry closure rebuilds it
        // from the owned bytes each attempt. A bad MIME makes `mime_str` fail
        // identically every call, so it short-circuits the retry loop as an
        // error rather than spinning. This POST is not idempotent: a 5xx
        // returned after the file was created server-side can leave an orphaned,
        // unattached file object on retry. That is acceptable here, the
        // motivating case is 429 (request rejected, safe to retry) and the
        // attach below is keyed on `external_id` with `overwrite: true`.
        let files_url = self.url("/v1/files");
        let resp = self
            .send_retrying(|| {
                let part = reqwest::multipart::Part::bytes(content.clone())
                    .file_name(file_name.to_owned())
                    .mime_str(mime)
                    .context(HttpSnafu)?;
                Ok(self
                    .http
                    .post(files_url.as_str())
                    .multipart(reqwest::multipart::Form::new().part("file", part)))
            })
            .await?;
        let created: CreatedFile = decode(resp).await?;

        let attach = AttachRequest {
            file_id: &created.id,
            external_id,
            overwrite: true,
            metadata,
        };
        let attach_url = self.url(&format!("/v1/stores/{store}/files"));
        let resp = self
            .send_retrying(|| Ok(self.http.post(attach_url.as_str()).json(&attach)))
            .await?;
        expect_ok(resp).await
    }

    /// Fetch one store file's indexing status by external id (or store file
    /// id). `Ok(None)` when the store holds no such file.
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn file_status(&self, store: &str, external_id: &str) -> Result<Option<FileStatus>> {
        let id = utf8_percent_encode(external_id, PATH_SEGMENT);
        let status_url = self.url(&format!("/v1/stores/{store}/files/{id}"));
        let resp = self
            .send_retrying(|| Ok(self.http.get(status_url.as_str())))
            .await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let object: FileObject = decode(resp).await?;
        Ok(Some(object.status))
    }

    /// Delete one file by external id (or store file id).
    ///
    /// # Errors
    /// Returns an error if the delete request fails.
    pub async fn delete_file(&self, store: &str, external_id: &str) -> Result<()> {
        let id = utf8_percent_encode(external_id, PATH_SEGMENT);
        let delete_url = self.url(&format!("/v1/stores/{store}/files/{id}"));
        let resp = self
            .send_retrying(|| Ok(self.http.delete(delete_url.as_str())))
            .await?;
        expect_ok(resp).await
    }

    /// Search one or more stores. `file_ids` further scopes matching chunks to
    /// (or away from) specific store files, `AND`ed with `filters`.
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn search(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&filter::Filter>,
        file_ids: Option<&FileIds>,
    ) -> Result<Vec<Chunk>> {
        let request = SearchRequest {
            query,
            store_identifiers: stores,
            top_k,
            search_options: options,
            filters,
            file_ids,
        };
        let search_url = self.url("/v1/stores/search");
        let resp = self
            .send_retrying(|| Ok(self.http.post(search_url.as_str()).json(&request)))
            .await?;
        let response: SearchResponse = decode(resp).await?;
        Ok(response.data.into_iter().map(Chunk::from).collect())
    }

    /// Grep one or more stores: run a regular expression over the same indexed
    /// chunks search covers, on the server's `/v1/stores/grep` endpoint.
    ///
    /// `pattern` is the regex; `top_k` caps the returned matches;
    /// `case_sensitive` toggles case folding server-side; `targets` selects
    /// which chunk fields to match against (e.g. `["text"]`). The response is
    /// decoded the same way [`search`](Self::search) decodes its chunks, so a
    /// grep hit and a search hit share the [`Chunk`] shape (line metadata may be
    /// absent and then deserializes as `None`).
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn grep(
        &self,
        stores: &[String],
        pattern: &str,
        top_k: usize,
        case_sensitive: bool,
        targets: &[&str],
        filters: Option<&filter::Filter>,
    ) -> Result<Vec<Chunk>> {
        let request = GrepRequest {
            store_identifiers: stores,
            pattern,
            top_k,
            case_sensitive,
            targets,
            filters,
        };
        let grep_url = self.url("/v1/stores/grep");
        let resp = self
            .send_retrying(|| Ok(self.http.post(grep_url.as_str()).json(&request)))
            .await?;
        let response: SearchResponse = decode(resp).await?;
        Ok(response.data.into_iter().map(Chunk::from).collect())
    }

    /// List chunks from one or more stores purely by metadata filters — no
    /// embeddings, no semantic similarity, no reranking — on the server's
    /// `/v1/stores/list-chunks` endpoint.
    ///
    /// This is the API for deterministic ranked retrieval over numeric
    /// metadata: combined with a `timestamp` range filter and
    /// `sort_by = SortBy::desc("timestamp")` it answers "what happened in the
    /// last N hours, newest first". `top_k` caps the returned chunks (the
    /// endpoint has no cursor; ask for more to get more). The response decodes
    /// the same way [`search`](Self::search) does, so a listed chunk and a
    /// search hit share the [`Chunk`] shape (`score` is meaningless here and
    /// arrives as the API's placeholder).
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn list_chunks(
        &self,
        stores: &[String],
        top_k: usize,
        filters: Option<&filter::Filter>,
        sort_by: Option<&SortBy>,
    ) -> Result<Vec<Chunk>> {
        let request = ListChunksRequest {
            store_identifiers: stores,
            top_k,
            filters,
            sort_by,
        };
        let list_url = self.url("/v1/stores/list-chunks");
        let resp = self
            .send_retrying(|| Ok(self.http.post(list_url.as_str()).json(&request)))
            .await?;
        let response: SearchResponse = decode(resp).await?;
        Ok(response.data.into_iter().map(Chunk::from).collect())
    }

    /// Ask a natural-language question against one or more stores. `file_ids`
    /// scopes the answering context like [`search`](Self::search); `qa` tunes
    /// the answering stage itself (citations, multimodal context,
    /// instructions — see [`QaOptions`]).
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    #[allow(
        clippy::too_many_arguments,
        reason = "thin pass-through of the endpoint's request surface"
    )]
    pub async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        qa: QaOptions,
        filters: Option<&filter::Filter>,
        file_ids: Option<&FileIds>,
    ) -> Result<AnswerResponse> {
        let request = QaRequest {
            search: SearchRequest {
                query,
                store_identifiers: stores,
                top_k,
                search_options: options,
                filters,
                file_ids,
            },
            qa_options: QaOptionsWire::from_options(&qa),
            instructions: qa.instructions.as_deref(),
        };
        let ask_url = self.url("/v1/stores/question-answering");
        let resp = self
            .send_retrying(|| Ok(self.http.post(ask_url.as_str()).json(&request)))
            .await?;
        let response: RawAnswerResponse = decode(resp).await?;
        Ok(AnswerResponse {
            answer: response.answer,
            sources: response.sources.into_iter().map(Chunk::from).collect(),
        })
    }

    /// Enhance a natural-language query against one or more stores on
    /// `/v1/stores/queries/enhance`: extract metadata filter conditions (and,
    /// for ranking-shaped queries like "newest shell commands", a metadata
    /// sort) from the query text. `instructions` optionally steers the
    /// extraction. The response's single item comes back as an
    /// [`EnhancedQuery`]; run the returned query/filter/sort yourself —
    /// enhancement performs no search.
    ///
    /// # Errors
    /// Returns an error if the request fails, cannot be decoded, or carries no
    /// item.
    pub async fn enhance_query(
        &self,
        stores: &[String],
        query: &str,
        instructions: Option<&str>,
    ) -> Result<EnhancedQuery> {
        let request = EnhanceRequest {
            query,
            store_identifiers: stores,
            instructions,
        };
        let enhance_url = self.url("/v1/stores/queries/enhance");
        let resp = self
            .send_retrying(|| Ok(self.http.post(enhance_url.as_str()).json(&request)))
            .await?;
        let response: EnhanceResponse = decode(resp).await?;
        response.items.into_iter().next().context(EnhanceEmptySnafu)
    }

    /// Rerank caller-supplied documents against a query on `/v1/reranking`.
    ///
    /// Unlike [`search`](Self::search), nothing is read from a store: the
    /// candidate texts come from the caller (e.g. lines piped into the `search`
    /// CLI) and only their ranking comes from the API. `model` names the
    /// reranking model (see [`DEFAULT_RERANK_MODEL`]); `top_k` caps the
    /// returned hits. The response is already sorted most-relevant first; each
    /// [`RerankHit::index`] points back into `input`, so the documents
    /// themselves are never echoed over the wire (`return_input: false`).
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn rerank(
        &self,
        model: &str,
        query: &str,
        input: &[String],
        top_k: usize,
    ) -> Result<Vec<RerankHit>> {
        let request = RerankRequest {
            model,
            query,
            input,
            top_k,
            return_input: false,
        };
        let rerank_url = self.url("/v1/reranking");
        let resp = self
            .send_retrying(|| Ok(self.http.post(rerank_url.as_str()).json(&request)))
            .await?;
        let response: RerankResponse = decode(resp).await?;
        Ok(response
            .data
            .into_iter()
            .map(|item| RerankHit {
                index: item.index,
                score: item.score,
            })
            .collect())
    }

    /// Fetch indexing progress for a store (pending and in-progress file
    /// counts). Zero on both means everything uploaded so far is searchable.
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn store_status(&self, store: &str) -> Result<StoreStatus> {
        let status_url = self.url(&format!("/v1/stores/{store}"));
        let resp = self
            .send_retrying(|| Ok(self.http.get(status_url.as_str())))
            .await?;
        let object: StoreObject = decode(resp).await?;
        Ok(StoreStatus {
            pending: object.file_counts.pending,
            in_progress: object.file_counts.in_progress,
        })
    }

    /// Count distinct metadata values across one or more stores on
    /// `/v1/stores/metadata-facets`. `facets` names the metadata keys to
    /// count (dot paths reach nested fields; an empty slice asks the server
    /// for every key, up to `limits.max_fields`); the result maps each key to
    /// its distinct values and the number of store files carrying each, and
    /// `filters` scopes which files are counted.
    ///
    /// Counts come from a bounded scan of at most `limits.max_files` store
    /// files, so a store larger than the bound reports truncated counts (see
    /// [`FACETS_MAX_FILES`]). This is the census primitive: one request
    /// answers "how many documents per source".
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn metadata_facets(
        &self,
        stores: &[String],
        facets: &[&str],
        filters: Option<&filter::Filter>,
        limits: FacetLimits,
    ) -> Result<Facets> {
        let request = FacetsRequest {
            store_identifiers: stores,
            facets,
            filters,
            max_fields: limits.max_fields,
            max_values_per_field: limits.max_values_per_field,
            max_files: limits.max_files,
        };
        let facets_url = self.url("/v1/stores/metadata-facets");
        let resp = self
            .send_retrying(|| Ok(self.http.post(facets_url.as_str()).json(&request)))
            .await?;
        let response: FacetsResponse = decode(resp).await?;
        Ok(response.facets)
    }

    /// Fetch a store's event histogram on
    /// `/v1/stores/{store}/events/histogram`: per-`bucket_seconds` bucket and
    /// per event type, how many events the store recorded between
    /// `start_time` and `end_time` (both RFC 3339). `event_types` restricts
    /// the histogram to those families; `None` includes ingestion, search,
    /// agentic search, and grep. This is the fleet-telemetry primitive: an
    /// empty ingestion bucket where uploads were expected is a stalled
    /// indexer.
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn events_histogram(
        &self,
        store: &str,
        start_time: &str,
        end_time: &str,
        bucket_seconds: u32,
        event_types: Option<&[EventType]>,
    ) -> Result<Vec<HistogramBucket>> {
        let request = HistogramRequest {
            start_time,
            end_time,
            bucket_seconds,
            event_types,
        };
        let id = utf8_percent_encode(store, PATH_SEGMENT);
        let histogram_url = self.url(&format!("/v1/stores/{id}/events/histogram"));
        let resp = self
            .send_retrying(|| Ok(self.http.post(histogram_url.as_str()).json(&request)))
            .await?;
        let response: HistogramResponse = decode(resp).await?;
        Ok(response.data)
    }
}

async fn decode<R: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<R> {
    if resp.status().is_success() {
        return resp.json::<R>().await.context(HttpSnafu);
    }
    Err(api_error(resp).await)
}

async fn expect_ok(resp: reqwest::Response) -> Result<()> {
    if resp.status().is_success() {
        return Ok(());
    }
    Err(api_error(resp).await)
}

async fn api_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    Error::Api { status, body }
}

/// Parse a `Retry-After` header in delta-seconds form, capped at [`BACKOFF_CAP`].
/// Mixedbread sends integer seconds; the HTTP-date form is not used by this API
/// and is ignored (we fall back to [`backoff`] then).
fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs: u64 = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs).min(BACKOFF_CAP))
}

/// Exponential backoff with equal jitter: half the capped exponential delay plus
/// a random fraction of the other half. Keeps the 16-in-flight uploads (and one
/// indexer per fleet host against one shared key) from retrying in lockstep and
/// re-colliding on the same rate limit.
fn backoff(attempt: u32) -> Duration {
    let exp = BACKOFF_BASE
        .saturating_mul(1u32 << attempt.min(5))
        .min(BACKOFF_CAP);
    let half = exp / 2;
    half + half.mul_f64(jitter_unit())
}

/// A cheap `[0.0, 1.0)` jitter value. Avoids a `rand` dependency: a process-wide
/// xorshift, seeded once from the wall clock, is plenty for spreading retry
/// timing across concurrent uploads. The load/store is not a CAS, but a lost
/// update only reuses a jitter value, which does not matter here.
fn jitter_unit() -> f64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        // Truncating the nanos to 64 bits is fine: this only seeds a jitter PRNG.
        #[allow(clippy::cast_possible_truncation)]
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        x = seed | 1;
    }
    // xorshift64
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    // `x >> 11` is a 53-bit integer and `2^53` is exactly representable, so the
    // f64 division is lossless despite the lint; the result is uniform in [0, 1).
    #[allow(clippy::cast_precision_loss)]
    {
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

#[derive(serde::Serialize)]
struct ListRequest<'a> {
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
    // The list endpoint takes its filter as `metadata_filter`, unlike
    // search/grep/question-answering, which take `filters`. The API silently
    // drops unknown body keys, so a filter sent as `filters` here returns the
    // whole store as if no filter was given (verified against the live API,
    // 2026-06-09).
    #[serde(rename = "metadata_filter", skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default)]
    data: Vec<ListItem>,
    #[serde(default)]
    pagination: Option<Pagination>,
}

#[derive(Deserialize)]
struct ListItem {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    external_id: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Deserialize)]
struct Pagination {
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    last_cursor: Option<String>,
}

#[derive(serde::Serialize)]
struct AttachRequest<'a> {
    file_id: &'a str,
    external_id: &'a str,
    overwrite: bool,
    metadata: serde_json::Value,
}

#[derive(Deserialize)]
struct CreatedFile {
    id: String,
}

/// The slice of the store-file object [`Client::file_status`] reads.
#[derive(Deserialize)]
struct FileObject {
    status: FileStatus,
}

#[derive(serde::Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    store_identifiers: &'a [String],
    top_k: usize,
    search_options: SearchOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
    // Request-level, not part of `search_options`: the API scopes by file ids
    // alongside (ANDed with) the metadata filter.
    #[serde(skip_serializing_if = "Option::is_none")]
    file_ids: Option<&'a FileIds>,
}

/// The `/v1/stores/question-answering` body: the shared search surface plus
/// the QA-only fields. The endpoint also defines `stream` (an SSE answer);
/// it is deliberately not modeled — see [`QaOptions`] for the live evidence.
#[derive(serde::Serialize)]
struct QaRequest<'a> {
    #[serde(flatten)]
    search: SearchRequest<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    qa_options: Option<QaOptionsWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
}

/// The `qa_options` body object (the API's `QuestionAnsweringOptions`).
/// Instructions are a sibling request-level field, not part of this object.
#[derive(serde::Serialize)]
struct QaOptionsWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    cite: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multimodal: Option<bool>,
}

impl QaOptionsWire {
    /// The wire object for `qa` — `None` when every toggle is unset, so the
    /// legacy body carries no `qa_options` key at all (the API treats absent
    /// and null differently for some fields).
    fn from_options(qa: &QaOptions) -> Option<Self> {
        (qa.cite.is_some() || qa.multimodal.is_some()).then_some(Self {
            cite: qa.cite,
            multimodal: qa.multimodal,
        })
    }
}

#[derive(serde::Serialize)]
struct EnhanceRequest<'a> {
    query: &'a str,
    store_identifiers: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
}

#[derive(Deserialize)]
struct EnhanceResponse {
    // The schema promises exactly one item; default-empty so a malformed empty
    // body surfaces as `Error::EnhanceEmpty` rather than a decode error.
    #[serde(default)]
    items: Vec<EnhancedQuery>,
}

#[derive(serde::Serialize)]
struct ListChunksRequest<'a> {
    store_identifiers: &'a [String],
    top_k: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_by: Option<&'a SortBy>,
}

#[derive(serde::Serialize)]
struct GrepRequest<'a> {
    store_identifiers: &'a [String],
    pattern: &'a str,
    top_k: usize,
    case_sensitive: bool,
    targets: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<RawChunk>,
}

#[derive(serde::Serialize)]
struct FacetsRequest<'a> {
    store_identifiers: &'a [String],
    // An empty facet list is omitted, which asks the server for every key.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    facets: &'a [&'a str],
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_fields: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_values_per_field: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_files: Option<u32>,
}

// The live response is `{"facets": {key: {value: count}}}` (verified
// 2026-06-12); the OpenAPI example shows a different `{count, value}` object
// per key, but the deployed server does not.
#[derive(Deserialize)]
struct FacetsResponse {
    #[serde(default)]
    facets: Facets,
}

#[derive(serde::Serialize)]
struct HistogramRequest<'a> {
    start_time: &'a str,
    end_time: &'a str,
    bucket_seconds: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_types: Option<&'a [EventType]>,
}

#[derive(Deserialize)]
struct HistogramResponse {
    #[serde(default)]
    data: Vec<HistogramBucket>,
}

#[derive(serde::Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    input: &'a [String],
    top_k: usize,
    return_input: bool,
}

#[derive(Deserialize)]
struct RerankResponse {
    #[serde(default)]
    data: Vec<RawRerankItem>,
}

#[derive(Deserialize)]
struct RawRerankItem {
    index: usize,
    #[serde(default)]
    score: f32,
}

// The public `AnswerResponse` is the projected shape; this is the raw wire
// shape it is built from.
#[derive(Deserialize)]
struct RawAnswerResponse {
    #[serde(default)]
    answer: String,
    #[serde(default)]
    sources: Vec<RawChunk>,
}

#[derive(Deserialize)]
struct StoreObject {
    #[serde(default)]
    file_counts: FileCounts,
}

#[derive(Default, Deserialize)]
struct FileCounts {
    #[serde(default)]
    pending: u64,
    #[serde(default)]
    in_progress: u64,
}

#[derive(Deserialize)]
struct RawChunk {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    score: f32,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    generated_metadata: Option<GeneratedMetadata>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GeneratedMetadata {
    #[serde(default)]
    start_line: Option<u32>,
    #[serde(default)]
    num_lines: Option<u32>,
}

impl From<RawChunk> for Chunk {
    fn from(raw: RawChunk) -> Self {
        let (start_line, num_lines) = raw
            .generated_metadata
            .map_or((None, None), |g| (g.start_line, g.num_lines));
        Self {
            text: raw.text,
            score: raw.score,
            filename: raw.filename,
            start_line,
            num_lines,
            metadata: raw.metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;

    use std::time::Duration;

    use super::{
        Agentic, AgenticConfig, BACKOFF_BASE, BACKOFF_CAP, Chunk, Client, DEFAULT_RERANK_MODEL,
        EnhancedQuery, Error, EventType, FACETS_MAX_FILES, FacetLimits, FileIds, FileStatus,
        HttpClient, ListChunksRequest, ListRequest, MAX_RETRIES, QaOptions, QaOptionsWire,
        QaRequest, RawChunk, Rerank, SearchOptions, SearchRequest, SortBy, backoff,
    };
    use crate::{Filter, Operator};

    #[test]
    fn list_request_sends_its_filter_as_metadata_filter() {
        // The list endpoint reads `metadata_filter`, not `filters`, and the API
        // silently ignores unknown keys: under the wrong name every "scoped"
        // listing was the whole store, which turned a per-source replace into
        // deletes of other sources' records. Pin the wire key.
        let filter = Filter::eq("source", "code");
        let request = ListRequest {
            limit: 100,
            after: None,
            filters: Some(&filter),
        };
        assert_eq!(
            serde_json::to_value(&request).expect("serialize"),
            serde_json::json!({
                "limit": 100,
                "metadata_filter": { "key": "source", "operator": "eq", "value": "code" }
            })
        );
    }

    #[test]
    fn sort_by_serializes_to_the_field_ascending_tuple() {
        // The API accepts `string | [field_path, ascending]`; we always emit
        // the tuple so the direction is explicit. Pin both directions.
        assert_eq!(
            serde_json::to_value(SortBy::desc("timestamp")).expect("serialize"),
            serde_json::json!(["timestamp", false])
        );
        assert_eq!(
            serde_json::to_value(SortBy::asc("generated_metadata.start_line")).expect("serialize"),
            serde_json::json!(["generated_metadata.start_line", true])
        );
    }

    #[test]
    fn list_chunks_request_matches_the_documented_wire_shape() {
        // Pins the `/v1/stores/list-chunks` body: store_identifiers, top_k, the
        // shared recursive `filters` (NOT `metadata_filter` — that is the
        // file-list endpoint's quirk), and the `[field, ascending]` sort tuple.
        let filter = Filter::condition("timestamp", Operator::Gte, 1_780_000_000_i64);
        let sort = SortBy::desc("timestamp");
        let request = ListChunksRequest {
            store_identifiers: &["index".to_owned()],
            top_k: 20,
            filters: Some(&filter),
            sort_by: Some(&sort),
        };
        assert_eq!(
            serde_json::to_value(&request).expect("serialize"),
            serde_json::json!({
                "store_identifiers": ["index"],
                "top_k": 20,
                "filters": { "key": "timestamp", "operator": "gte", "value": 1_780_000_000_i64 },
                "sort_by": ["timestamp", false],
            })
        );
        // Unset filter/sort are omitted entirely (the API treats absent and
        // null differently for some fields; never send nulls).
        let bare = ListChunksRequest {
            store_identifiers: &["index".to_owned()],
            top_k: 5,
            filters: None,
            sort_by: None,
        };
        assert_eq!(
            serde_json::to_value(&bare).expect("serialize"),
            serde_json::json!({ "store_identifiers": ["index"], "top_k": 5 })
        );
    }

    #[tokio::test]
    async fn list_chunks_posts_the_endpoint_and_decodes_chunks() {
        // Round-trip through a real router: the request must hit
        // `/v1/stores/list-chunks` and the response decodes through the same
        // RawChunk -> Chunk projection search uses.
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::default();
        let app = Router::new().route(
            "/v1/stores/list-chunks",
            axum::routing::post({
                let captured = Arc::clone(&captured);
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    *captured.lock().expect("lock") = Some(body);
                    async {
                        (
                            StatusCode::OK,
                            r#"{"data":[{"text":"gt sync","score":1.0,"metadata":{"source":"shell","timestamp":1781248268}}]}"#,
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let sort = SortBy::desc("timestamp");
        let chunks = client
            .list_chunks(&["index".to_owned()], 1, None, Some(&sort))
            .await
            .expect("list chunks");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text.as_deref(), Some("gt sync"));
        assert_eq!(
            chunks[0]
                .metadata
                .as_ref()
                .and_then(|m| m.get("timestamp"))
                .and_then(serde_json::Value::as_i64),
            Some(1_781_248_268)
        );
        assert_eq!(
            captured.lock().expect("lock").take().expect("request body"),
            serde_json::json!({
                "store_identifiers": ["index"],
                "top_k": 1,
                "sort_by": ["timestamp", false],
            })
        );
    }

    #[test]
    fn rerank_serializes_as_bool_or_object() {
        // The toggle keeps the legacy bare-bool wire body byte-for-byte.
        assert_eq!(
            serde_json::to_value(Rerank::off()).expect("serialize"),
            serde_json::json!(false)
        );
        assert_eq!(
            serde_json::to_value(Rerank::server_default()).expect("serialize"),
            serde_json::json!(true)
        );
        // A pinned model serializes to the `{ "model": ... }` object form,
        // with no `top_k` key when unset.
        assert_eq!(
            serde_json::to_value(Rerank::listwise()).expect("serialize"),
            serde_json::json!({ "model": DEFAULT_RERANK_MODEL })
        );
        // A capped rerank adds `top_k` (the API's RerankConfig).
        assert_eq!(
            serde_json::to_value(Rerank::Model {
                model: DEFAULT_RERANK_MODEL.to_owned(),
                top_k: Some(5),
            })
            .expect("serialize"),
            serde_json::json!({ "model": DEFAULT_RERANK_MODEL, "top_k": 5 })
        );
    }

    #[test]
    fn agentic_serializes_as_bool_or_config_object() {
        // The toggle keeps the legacy bare-bool body; a config serializes to
        // the AgenticSearchConfig object with unset fields omitted (the API
        // treats absent and null differently for some fields).
        assert_eq!(
            serde_json::to_value(Agentic::off()).expect("serialize"),
            serde_json::json!(false)
        );
        assert_eq!(
            serde_json::to_value(Agentic::on()).expect("serialize"),
            serde_json::json!(true)
        );
        assert_eq!(
            serde_json::to_value(Agentic::Config(AgenticConfig {
                max_rounds: Some(2),
                instructions: Some("prefer recent records".to_owned()),
                ..AgenticConfig::default()
            }))
            .expect("serialize"),
            serde_json::json!({ "max_rounds": 2, "instructions": "prefer recent records" })
        );

        assert!(!Agentic::off().is_enabled());
        assert!(Agentic::on().is_enabled());
        assert!(Agentic::Config(AgenticConfig::default()).is_enabled());
    }

    #[test]
    fn search_request_matches_the_documented_wire_shape() {
        // Pins the `/v1/stores/search` body across every option this client
        // models: the StoreChunkSearchOptions fields live under
        // `search_options`, while `file_ids` is a sibling of `filters` at the
        // request level (verified against the live API, 2026-06-12).
        let filter = Filter::eq("source", "code");
        let file_ids = FileIds::exclude(vec!["2b5d7a52".to_owned()]);
        let request = SearchRequest {
            query: "upload",
            store_identifiers: &["index".to_owned()],
            top_k: 3,
            search_options: SearchOptions {
                rerank: Rerank::Model {
                    model: DEFAULT_RERANK_MODEL.to_owned(),
                    top_k: Some(2),
                },
                agentic: Agentic::off(),
                score_threshold: None,
                return_metadata: Some(true),
                rewrite_query: Some(true),
                apply_search_rules: Some(false),
            },
            filters: Some(&filter),
            file_ids: Some(&file_ids),
        };
        assert_eq!(
            serde_json::to_value(&request).expect("serialize"),
            serde_json::json!({
                "query": "upload",
                "store_identifiers": ["index"],
                "top_k": 3,
                "search_options": {
                    "rerank": { "model": DEFAULT_RERANK_MODEL, "top_k": 2 },
                    "agentic": false,
                    "return_metadata": true,
                    "rewrite_query": true,
                    "apply_search_rules": false,
                },
                "filters": { "key": "source", "operator": "eq", "value": "code" },
                "file_ids": ["not_in", ["2b5d7a52"]],
            })
        );

        // With every new knob unset the legacy wire body is unchanged: no
        // rewrite_query/apply_search_rules/file_ids keys at all.
        let legacy = SearchRequest {
            query: "upload",
            store_identifiers: &["index".to_owned()],
            top_k: 3,
            search_options: SearchOptions {
                rerank: Rerank::off(),
                agentic: Agentic::off(),
                score_threshold: None,
                return_metadata: None,
                rewrite_query: None,
                apply_search_rules: None,
            },
            filters: None,
            file_ids: None,
        };
        assert_eq!(
            serde_json::to_value(&legacy).expect("serialize"),
            serde_json::json!({
                "query": "upload",
                "store_identifiers": ["index"],
                "top_k": 3,
                "search_options": { "rerank": false, "agentic": false },
            })
        );
    }

    #[test]
    fn qa_request_matches_the_documented_wire_shape() {
        // Pins the `/v1/stores/question-answering` body: the flattened search
        // surface plus `qa_options` (the API's QuestionAnsweringOptions) and
        // the request-level `instructions` (verified against the live API,
        // 2026-06-12).
        let request = QaRequest {
            search: SearchRequest {
                query: "what bucket does the indexer use",
                store_identifiers: &["index".to_owned()],
                top_k: 3,
                search_options: SearchOptions {
                    rerank: Rerank::server_default(),
                    agentic: Agentic::off(),
                    score_threshold: None,
                    return_metadata: Some(true),
                    rewrite_query: None,
                    apply_search_rules: None,
                },
                filters: None,
                file_ids: None,
            },
            qa_options: QaOptionsWire::from_options(&QaOptions {
                cite: Some(true),
                multimodal: Some(false),
                instructions: None,
            }),
            instructions: Some("answer in one sentence"),
        };
        assert_eq!(
            serde_json::to_value(&request).expect("serialize"),
            serde_json::json!({
                "query": "what bucket does the indexer use",
                "store_identifiers": ["index"],
                "top_k": 3,
                "search_options": { "rerank": true, "agentic": false, "return_metadata": true },
                "qa_options": { "cite": true, "multimodal": false },
                "instructions": "answer in one sentence",
            })
        );

        // Default QA options leave the legacy wire body untouched: no
        // `qa_options` or `instructions` keys at all.
        let legacy = QaRequest {
            search: SearchRequest {
                query: "q",
                store_identifiers: &["index".to_owned()],
                top_k: 1,
                search_options: SearchOptions {
                    rerank: Rerank::off(),
                    agentic: Agentic::off(),
                    score_threshold: None,
                    return_metadata: None,
                    rewrite_query: None,
                    apply_search_rules: None,
                },
                filters: None,
                file_ids: None,
            },
            qa_options: QaOptionsWire::from_options(&QaOptions::default()),
            instructions: None,
        };
        assert_eq!(
            serde_json::to_value(&legacy).expect("serialize"),
            serde_json::json!({
                "query": "q",
                "store_identifiers": ["index"],
                "top_k": 1,
                "search_options": { "rerank": false, "agentic": false },
            })
        );

        // One set toggle is enough to carry the object, without a null for
        // the unset sibling.
        assert_eq!(
            serde_json::to_value(QaOptionsWire::from_options(&QaOptions {
                cite: Some(false),
                multimodal: None,
                instructions: None,
            }))
            .expect("serialize"),
            serde_json::json!({ "cite": false })
        );
    }

    #[test]
    fn file_ids_serialize_as_bare_list_or_operator_tuple() {
        assert_eq!(
            serde_json::to_value(FileIds::include(vec!["a".to_owned(), "b".to_owned()]))
                .expect("serialize"),
            serde_json::json!(["a", "b"])
        );
        assert_eq!(
            serde_json::to_value(FileIds::Scoped(Operator::In, vec!["a".to_owned()]))
                .expect("serialize"),
            serde_json::json!(["in", ["a"]])
        );
        assert_eq!(
            serde_json::to_value(FileIds::exclude(vec!["a".to_owned()])).expect("serialize"),
            serde_json::json!(["not_in", ["a"]])
        );
    }

    #[tokio::test]
    async fn list_files_decodes_id_external_id_metadata_and_created_at() {
        // Pins the slice of the store-file listing the GC pass depends on: the
        // file object's own `id` (the only unambiguous delete handle when two
        // objects share an external id) and `created_at` (what "keep the
        // newest" orders by) must survive the projection into StoredFile, and
        // a sparse item (no id, no timestamp) must decode rather than error.
        let app = Router::new().route(
            "/v1/stores/{store}/files/list",
            axum::routing::post(|| async {
                (
                    StatusCode::OK,
                    r#"{"data":[
                        {"id":"f-1","external_id":"linear:issue:A",
                         "metadata":{"source":"linear","content_hash":"sha256:aa"},
                         "created_at":"2026-06-11T00:00:00Z"},
                        {"external_id":"legacy"}
                    ]}"#,
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let files = client.list_files("s", None).await.expect("list");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].id.as_deref(), Some("f-1"));
        assert_eq!(files[0].external_id.as_deref(), Some("linear:issue:A"));
        assert_eq!(
            files[0].created_at.as_deref(),
            Some("2026-06-11T00:00:00Z")
        );
        assert_eq!(
            files[0]
                .metadata
                .as_ref()
                .and_then(|m| m.get("content_hash"))
                .and_then(serde_json::Value::as_str),
            Some("sha256:aa")
        );
        assert_eq!(files[1].id, None);
        assert_eq!(files[1].created_at, None);
    }

    #[tokio::test]
    async fn enhance_posts_the_endpoint_and_decodes_the_single_item() {
        // Round-trip through a real router: the request must hit
        // `/v1/stores/queries/enhance` with the documented body, and the
        // response's one item decodes through the tagged EnhancedQuery enum.
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::default();
        let app = Router::new().route(
            "/v1/stores/queries/enhance",
            axum::routing::post({
                let captured = Arc::clone(&captured);
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    *captured.lock().expect("lock") = Some(body);
                    async {
                        (
                            StatusCode::OK,
                            r#"{"items":[{"type":"query","query":"indexer slack messages","metadata_filters":[{"key":"source","operator":"eq","value":"slack"}],"filter_mode":"all","rank_by":null,"direction":null}]}"#,
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let enhanced = client
            .enhance_query(
                &["index".to_owned()],
                "slack messages about the indexer",
                Some("prefer source filters"),
            )
            .await
            .expect("enhance");
        let EnhancedQuery::Query { query, .. } = &enhanced else {
            panic!("expected query item, got {enhanced:?}");
        };
        assert_eq!(query, "indexer slack messages");
        assert_eq!(
            serde_json::to_value(enhanced.filter().expect("filter")).expect("serialize"),
            serde_json::json!({ "key": "source", "operator": "eq", "value": "slack" })
        );
        assert_eq!(
            captured.lock().expect("lock").take().expect("request body"),
            serde_json::json!({
                "query": "slack messages about the indexer",
                "store_identifiers": ["index"],
                "instructions": "prefer source filters",
            })
        );
    }

    #[tokio::test]
    async fn metadata_facets_posts_the_endpoint_and_decodes_value_counts() {
        // Round-trip through a real router: the request must hit
        // `/v1/stores/metadata-facets` with the documented body (facet keys,
        // scan caps, no nulls for unset caps) and the response decodes the
        // live `{key: {value: count}}` shape.
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::default();
        let app = Router::new().route(
            "/v1/stores/metadata-facets",
            axum::routing::post({
                let captured = Arc::clone(&captured);
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    *captured.lock().expect("lock") = Some(body);
                    async {
                        (
                            StatusCode::OK,
                            r#"{"facets":{"source":{"shell":1154,"code":9644}}}"#,
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let facets = client
            .metadata_facets(
                &["index".to_owned()],
                &["source"],
                None,
                FacetLimits {
                    max_values_per_field: Some(256),
                    max_files: Some(FACETS_MAX_FILES),
                    ..FacetLimits::default()
                },
            )
            .await
            .expect("facets");
        assert_eq!(facets["source"]["shell"], 1_154);
        assert_eq!(facets["source"]["code"], 9_644);
        assert_eq!(
            captured.lock().expect("lock").take().expect("request body"),
            serde_json::json!({
                "store_identifiers": ["index"],
                "facets": ["source"],
                "max_values_per_field": 256,
                "max_files": 100_000,
            })
        );
    }

    #[tokio::test]
    async fn events_histogram_posts_the_store_path_and_decodes_buckets() {
        // Round-trip through a real router: the store name must land as one
        // path segment of `/v1/stores/{store}/events/histogram`, the body
        // must carry the window/bucket/types, and the buckets decode with
        // `type` mapped onto `event_type`.
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::default();
        let app = Router::new().route(
            "/v1/stores/{store}/events/histogram",
            axum::routing::post({
                let captured = Arc::clone(&captured);
                move |axum::extract::Path(store): axum::extract::Path<String>,
                      axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    *captured.lock().expect("lock") =
                        Some(serde_json::json!({ "store": store, "body": body }));
                    async {
                        (
                            StatusCode::OK,
                            r#"{"object":"store.histogram","data":[{"bucket_start":"2026-06-12T00:00:00Z","bucket_end":"2026-06-12T12:00:00Z","type":"store.file.completed","event_count":6407}]}"#,
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let buckets = client
            .events_histogram(
                "index",
                "2026-06-12T00:00:00Z",
                "2026-06-13T00:00:00Z",
                43_200,
                Some(&[EventType::Ingestion, EventType::AgenticSearch]),
            )
            .await
            .expect("histogram");
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].event_type, "store.file.completed");
        assert_eq!(buckets[0].event_count, 6_407);
        assert_eq!(buckets[0].bucket_start, "2026-06-12T00:00:00Z");
        assert_eq!(
            captured.lock().expect("lock").take().expect("request"),
            serde_json::json!({
                "store": "index",
                "body": {
                    "start_time": "2026-06-12T00:00:00Z",
                    "end_time": "2026-06-13T00:00:00Z",
                    "bucket_seconds": 43_200,
                    "event_types": ["ingestion", "agentic_search"],
                },
            })
        );
    }

    /// A spawned test server: its base URL and a shared counter of requests it received.
    struct MockServer {
        base_url: String,
        calls: Arc<AtomicUsize>,
    }

    #[test]
    fn raw_chunk_projects_generated_metadata() {
        let json = serde_json::json!({
            "text": "fn main() {}",
            "score": 0.9,
            "filename": "main.rs",
            "generated_metadata": { "start_line": 4, "num_lines": 2 },
            "metadata": { "path": "src/main.rs", "hash": "sha256:abc" }
        });
        let raw: RawChunk = serde_json::from_value(json).expect("parse");
        let chunk = Chunk::from(raw);

        assert_eq!(chunk.start_line, Some(4));
        assert_eq!(chunk.num_lines, Some(2));
        assert_eq!(chunk.text.as_deref(), Some("fn main() {}"));
        assert_eq!(
            chunk
                .metadata
                .and_then(|m| m.get("hash").and_then(|h| h.as_str()).map(str::to_owned)),
            Some("sha256:abc".to_owned())
        );
    }

    #[test]
    fn missing_generated_metadata_is_none() {
        let raw: RawChunk =
            serde_json::from_value(serde_json::json!({ "score": 0.1 })).expect("parse");
        let chunk = Chunk::from(raw);
        assert_eq!(chunk.start_line, None);
        assert_eq!(chunk.num_lines, None);
    }

    #[test]
    fn backoff_stays_within_equal_jitter_band_and_cap() {
        for attempt in 0..10u32 {
            let exp = BACKOFF_BASE
                .saturating_mul(1u32 << attempt.min(5))
                .min(BACKOFF_CAP);
            let delay = backoff(attempt);
            assert!(
                delay >= exp / 2,
                "attempt {attempt}: {delay:?} below half {:?}",
                exp / 2
            );
            assert!(
                delay <= exp,
                "attempt {attempt}: {delay:?} above exp {exp:?}"
            );
            assert!(
                delay <= BACKOFF_CAP,
                "attempt {attempt}: {delay:?} above cap"
            );
        }
    }

    /// Mock server that answers `429` (with `Retry-After: 0` so retries are
    /// instant) for the first `fail_times` requests, then `200`. Returns the
    /// base URL and a shared counter of total requests received.
    async fn spawn_mock(fail_times: usize) -> MockServer {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new().fallback({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    if n < fail_times {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            [(header::RETRY_AFTER, "0")],
                            "slow down",
                        )
                            .into_response()
                    } else {
                        (StatusCode::OK, "{}").into_response()
                    }
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        MockServer {
            base_url: format!("http://{addr}"),
            calls,
        }
    }

    #[tokio::test]
    async fn delete_file_sends_a_slashed_id_as_one_path_segment() {
        // An external id may contain `/` (e.g. `github:org/repo`). Unencoded it
        // splits the path and the API 404s; this routes through a real router,
        // so a regression fails to match the `{file}` segment at all.
        let captured: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
        let app = Router::new().route(
            "/v1/stores/{store}/files/{file}",
            axum::routing::delete({
                let captured = Arc::clone(&captured);
                move |axum::extract::Path((_store, file)): axum::extract::Path<(String, String)>| {
                    *captured.lock().expect("lock") = Some(file);
                    async { (StatusCode::OK, "{}") }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        client
            .delete_file("s", "github:indexable-inc/index")
            .await
            .expect("delete routes as one segment");
        assert_eq!(
            captured.lock().expect("lock").as_deref(),
            Some("github:indexable-inc/index"),
            "the router must decode the segment back to the original id"
        );
    }

    #[tokio::test]
    async fn file_status_reads_the_status_field_and_maps_404_to_none() {
        // Pins the per-file status wire contract: GET the store-file object,
        // read its `status` string (including one this client has never heard
        // of), and fold a 404 into `None` rather than an error — a file deleted
        // out from under a waiting caller is settled, not a failure.
        let app = Router::new().route(
            "/v1/stores/{store}/files/{file}",
            axum::routing::get(
                |axum::extract::Path((_store, file)): axum::extract::Path<(String, String)>| async move {
                    match file.as_str() {
                        "queued" => (StatusCode::OK, r#"{"status":"pending"}"#),
                        "embedding" => (StatusCode::OK, r#"{"status":"in_progress"}"#),
                        "done" => (StatusCode::OK, r#"{"status":"completed"}"#),
                        "novel" => (StatusCode::OK, r#"{"status":"some_future_state"}"#),
                        _ => (StatusCode::NOT_FOUND, r#"{"error":"not found"}"#),
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let status = |id: &'static str| client.file_status("s", id);
        assert_eq!(status("queued").await.expect("get"), Some(FileStatus::Pending));
        assert_eq!(
            status("embedding").await.expect("get"),
            Some(FileStatus::InProgress)
        );
        assert_eq!(status("done").await.expect("get"), Some(FileStatus::Completed));
        assert_eq!(status("novel").await.expect("get"), Some(FileStatus::Unknown));
        assert_eq!(status("gone").await.expect("get"), None);

        // The waiting contract: only the two live states keep a poll loop going.
        assert!(!FileStatus::Pending.is_settled());
        assert!(!FileStatus::InProgress.is_settled());
        assert!(FileStatus::Completed.is_settled());
        assert!(FileStatus::Failed.is_settled());
        assert!(FileStatus::Unknown.is_settled());
    }

    #[tokio::test]
    async fn rerank_posts_documents_and_projects_index_score() {
        // Pins the /v1/reranking wire contract: the request carries the model,
        // query, documents (`input`), top_k, and `return_input: false`; the
        // response's `data` items project to (index, score) pairs pointing back
        // into the submitted slice.
        let captured: Arc<std::sync::Mutex<Option<serde_json::Value>>> = Arc::default();
        let app = Router::new().route(
            "/v1/reranking",
            axum::routing::post({
                let captured = Arc::clone(&captured);
                move |axum::extract::Json(body): axum::extract::Json<serde_json::Value>| {
                    *captured.lock().expect("lock") = Some(body);
                    async {
                        (
                            StatusCode::OK,
                            r#"{"data":[{"index":2,"score":0.91},{"index":0,"score":0.12}]}"#,
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let client = Client::new(format!("http://{addr}"), "test-key").expect("client");
        let docs = vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
        let hits = client
            .rerank(DEFAULT_RERANK_MODEL, "which greek letter", &docs, 2)
            .await
            .expect("rerank");
        assert_eq!(
            hits.iter().map(|h| h.index).collect::<Vec<_>>(),
            vec![2, 0],
            "hits keep the API's most-relevant-first order"
        );
        assert!((hits[0].score - 0.91).abs() < 1e-6, "{}", hits[0].score);
        assert_eq!(
            captured.lock().expect("lock").take().expect("request body"),
            serde_json::json!({
                "model": DEFAULT_RERANK_MODEL,
                "query": "which greek letter",
                "input": ["alpha", "beta", "gamma"],
                "top_k": 2,
                "return_input": false,
            })
        );
    }

    #[tokio::test]
    async fn retries_429_then_succeeds() {
        let MockServer { base_url, calls } = spawn_mock(2).await;
        let client = Client::new(base_url, "test-key").expect("client");
        client
            .ensure_store("store")
            .await
            .expect("succeeds after retries");
        // 2 rejected + 1 accepted.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// TCP server that drops the connection without a response for the first
    /// `fail_times` requests (a transport error, no HTTP status), then answers
    /// `200`. Returns the base URL and a counter of accepted connections.
    async fn spawn_flaky_tcp(fail_times: usize) -> MockServer {
        use tokio::io::AsyncWriteExt as _;

        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let calls_task = Arc::clone(&calls);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    continue;
                };
                let n = calls_task.fetch_add(1, Ordering::SeqCst);
                if n < fail_times {
                    // Close immediately: the client sees the connection drop
                    // before a response, i.e. a transport error.
                    drop(sock);
                } else {
                    let _ = sock
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                        .await;
                    let _ = sock.shutdown().await;
                }
            }
        });
        MockServer {
            base_url: format!("http://{addr}"),
            calls,
        }
    }

    /// TCP server that accepts and then holds the connection open without ever
    /// responding for the first `stall_times` connections, then answers `200`.
    /// This is the wedge [`super::REQUEST_TIMEOUT`] exists for: the connection
    /// stays ESTAB, no transport error fires on its own, and only a client-side
    /// timeout can turn the stall into a retryable error. Returns the base URL
    /// and a counter of accepted connections.
    async fn spawn_stalling_tcp(stall_times: usize) -> MockServer {
        use tokio::io::AsyncWriteExt as _;

        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let calls_task = Arc::clone(&calls);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    continue;
                };
                let n = calls_task.fetch_add(1, Ordering::SeqCst);
                // Each connection gets its own task: a stalled socket must stay
                // open (dropping it would be a transport error, a different
                // test) without blocking the accept loop.
                tokio::spawn(async move {
                    if n < stall_times {
                        tokio::time::sleep(Duration::from_hours(1)).await;
                    } else {
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                            .await;
                        let _ = sock.shutdown().await;
                    }
                });
            }
        });
        MockServer {
            base_url: format!("http://{addr}"),
            calls,
        }
    }

    #[tokio::test]
    async fn a_stalled_response_times_out_and_retries() {
        // Pays real backoff (~1s), like the transport-error test below.
        let MockServer { base_url, calls } = spawn_stalling_tcp(2).await;
        // Built directly rather than via `Client::new` so the stall is bounded
        // by a test-sized timeout instead of the production REQUEST_TIMEOUT;
        // the retry path under test is identical.
        let client = Client {
            http: HttpClient::builder()
                .timeout(Duration::from_millis(200))
                .build()
                .expect("client"),
            base_url,
            api_key: "test-key".into(),
        };
        client
            .ensure_store("store")
            .await
            .expect("succeeds after stalled attempts time out");
        // 2 stalled connections + 1 answered.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retries_transport_errors_then_succeeds() {
        // Pays real backoff (~1s): transport errors carry no `Retry-After`, so
        // don't raise `fail_times` expecting it to stay instant.
        let MockServer { base_url, calls } = spawn_flaky_tcp(2).await;
        let client = Client::new(base_url, "test-key").expect("client");
        client
            .ensure_store("store")
            .await
            .expect("succeeds after transport retries");
        // 2 dropped connections + 1 accepted.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let MockServer { base_url, calls } = spawn_mock(usize::MAX).await;
        let client = Client::new(base_url, "test-key").expect("client");
        let err = client
            .ensure_store("store")
            .await
            .expect_err("never succeeds");
        assert!(matches!(err, Error::Api { status: 429, .. }), "got {err:?}");
        // The initial attempt plus MAX_RETRIES retries.
        assert_eq!(calls.load(Ordering::SeqCst), (MAX_RETRIES + 1) as usize);
    }
}
