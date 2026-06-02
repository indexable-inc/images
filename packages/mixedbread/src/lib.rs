//! Minimal async client for the [Mixedbread](https://www.mixedbread.com) vector
//! store API. It owns HTTP and JSON shapes only; it carries no domain logic, so
//! it can back a search tool or any other consumer.
//!
//! Endpoints covered: store create/get (`/v1/stores`), the two-step file upload
//! (`/v1/files` then `/v1/stores/{store}/files`), file listing and deletion,
//! search (`/v1/stores/search`), regex grep (`/v1/stores/grep`), and
//! question-answering (`/v1/stores/question-answering`).

use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Client as HttpClient, StatusCode};
use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu};

pub mod auth;
pub mod filter;

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
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Search tuning forwarded to the API.
///
/// `score_threshold` and `return_metadata` are skipped when unset, so a caller
/// that only sets `rerank`/`agentic` produces the same wire body as before.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SearchOptions {
    /// Apply the second-stage reranker.
    pub rerank: bool,
    /// Let the API plan and run multiple searches.
    pub agentic: bool,
    /// Drop hits scoring below this threshold (`0.0..=1.0`). Used to keep a
    /// low-relevance source from crowding a multi-source result list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
    /// Ask the API to return each chunk's file metadata, so a result can be
    /// mapped back to its source. Skipped when `None` (API default applies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_metadata: Option<bool>,
}

/// A file as reported by the store's file listing.
#[derive(Debug, Clone)]
pub struct StoredFile {
    /// Caller-assigned external id, if any.
    pub external_id: Option<String>,
    /// Arbitrary metadata attached at upload time.
    pub metadata: Option<serde_json::Value>,
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

/// Indexing progress for a store: how many files are still being processed.
#[derive(Debug, Clone, Copy)]
pub struct StoreStatus {
    /// Files queued but not yet processed.
    pub pending: u64,
    /// Files currently being embedded.
    pub in_progress: u64,
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
        let http = HttpClient::builder().build().context(BuildClientSnafu)?;
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
                    external_id: item.external_id,
                    metadata: item.metadata,
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

    /// Delete one file by external id (or store file id).
    ///
    /// # Errors
    /// Returns an error if the delete request fails.
    pub async fn delete_file(&self, store: &str, external_id: &str) -> Result<()> {
        let delete_url = self.url(&format!("/v1/stores/{store}/files/{external_id}"));
        let resp = self
            .send_retrying(|| Ok(self.http.delete(delete_url.as_str())))
            .await?;
        expect_ok(resp).await
    }

    /// Search one or more stores.
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
    ) -> Result<Vec<Chunk>> {
        let request = SearchRequest {
            query,
            store_identifiers: stores,
            top_k,
            search_options: options,
            filters,
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

    /// Ask a natural-language question against one or more stores.
    ///
    /// # Errors
    /// Returns an error if the request fails or cannot be decoded.
    pub async fn ask(
        &self,
        stores: &[String],
        query: &str,
        top_k: usize,
        options: SearchOptions,
        filters: Option<&filter::Filter>,
    ) -> Result<AnswerResponse> {
        let request = SearchRequest {
            query,
            store_identifiers: stores,
            top_k,
            search_options: options,
            filters,
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
    external_id: Option<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
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

#[derive(serde::Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    store_identifiers: &'a [String],
    top_k: usize,
    search_options: SearchOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    filters: Option<&'a filter::Filter>,
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

    use super::{BACKOFF_BASE, BACKOFF_CAP, Chunk, Client, Error, MAX_RETRIES, RawChunk, backoff};

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
            assert!(delay >= exp / 2, "attempt {attempt}: {delay:?} below half {:?}", exp / 2);
            assert!(delay <= exp, "attempt {attempt}: {delay:?} above exp {exp:?}");
            assert!(delay <= BACKOFF_CAP, "attempt {attempt}: {delay:?} above cap");
        }
    }

    /// Mock server that answers `429` (with `Retry-After: 0` so retries are
    /// instant) for the first `fail_times` requests, then `200`. Returns the
    /// base URL and a shared counter of total requests received.
    async fn spawn_mock(fail_times: usize) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new().fallback({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst);
                    if n < fail_times {
                        (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "0")], "slow down")
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
        (format!("http://{addr}"), calls)
    }

    #[tokio::test]
    async fn retries_429_then_succeeds() {
        let (base, calls) = spawn_mock(2).await;
        let client = Client::new(base, "test-key").expect("client");
        client.ensure_store("store").await.expect("succeeds after retries");
        // 2 rejected + 1 accepted.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// TCP server that drops the connection without a response for the first
    /// `fail_times` requests (a transport error, no HTTP status), then answers
    /// `200`. Returns the base URL and a counter of accepted connections.
    async fn spawn_flaky_tcp(fail_times: usize) -> (String, Arc<AtomicUsize>) {
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
        (format!("http://{addr}"), calls)
    }

    #[tokio::test]
    async fn retries_transport_errors_then_succeeds() {
        // Pays real backoff (~1s): transport errors carry no `Retry-After`, so
        // don't raise `fail_times` expecting it to stay instant.
        let (base, calls) = spawn_flaky_tcp(2).await;
        let client = Client::new(base, "test-key").expect("client");
        client
            .ensure_store("store")
            .await
            .expect("succeeds after transport retries");
        // 2 dropped connections + 1 accepted.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        let (base, calls) = spawn_mock(usize::MAX).await;
        let client = Client::new(base, "test-key").expect("client");
        let err = client.ensure_store("store").await.expect_err("never succeeds");
        assert!(matches!(err, Error::Api { status: 429, .. }), "got {err:?}");
        // The initial attempt plus MAX_RETRIES retries.
        assert_eq!(calls.load(Ordering::SeqCst), (MAX_RETRIES + 1) as usize);
    }
}
