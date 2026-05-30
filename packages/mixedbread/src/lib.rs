//! Minimal async client for the [Mixedbread](https://www.mixedbread.com) vector
//! store API. It owns HTTP and JSON shapes only; it carries no domain logic, so
//! it can back a semantic-search tool or any other consumer.
//!
//! Endpoints covered: store create/get (`/v1/stores`), the two-step file upload
//! (`/v1/files` then `/v1/stores/{store}/files`), file listing and deletion,
//! search (`/v1/stores/search`), and question-answering
//! (`/v1/stores/question-answering`).

use std::collections::HashSet;
use std::path::PathBuf;

use reqwest::{Client as HttpClient, StatusCode};
use serde::Deserialize;
use snafu::{OptionExt as _, ResultExt as _, Snafu};

pub mod auth;

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://api.mixedbread.com";

/// Environment variable holding the API key.
pub const API_KEY_ENV: &str = "MXBAI_API_KEY";

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
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SearchOptions {
    /// Apply the second-stage reranker.
    pub rerank: bool,
    /// Let the API plan and run multiple searches.
    pub agentic: bool,
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

    /// Ensure the named store exists, creating it if absent.
    ///
    /// # Errors
    /// Returns an error if the store cannot be fetched or created.
    pub async fn ensure_store(&self, name: &str) -> Result<()> {
        let resp = self
            .http
            .get(self.url(&format!("/v1/stores/{name}")))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context(HttpSnafu)?;
        if resp.status().is_success() {
            return Ok(());
        }
        if resp.status() != StatusCode::NOT_FOUND {
            return Err(api_error(resp).await);
        }
        let created = self
            .http
            .post(self.url("/v1/stores"))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .context(HttpSnafu)?;
        expect_ok(created).await
    }

    /// List files in a store, following cursor pagination.
    ///
    /// # Errors
    /// Returns an error if any page request fails or cannot be decoded.
    pub async fn list_files(&self, store: &str) -> Result<Vec<StoredFile>> {
        let mut files = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let request = ListRequest {
                limit: 100,
                after: after.as_deref(),
            };
            let resp = self
                .http
                .post(self.url(&format!("/v1/stores/{store}/files/list")))
                .bearer_auth(&self.api_key)
                .json(&request)
                .send()
                .await
                .context(HttpSnafu)?;
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

    /// Convenience over [`list_files`](Self::list_files): collect the set of
    /// non-null external ids.
    ///
    /// # Errors
    /// Returns an error if the listing fails.
    pub async fn list_external_ids(&self, store: &str) -> Result<HashSet<String>> {
        Ok(self
            .list_files(store)
            .await?
            .into_iter()
            .filter_map(|file| file.external_id)
            .collect())
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
        metadata: serde_json::Value,
    ) -> Result<()> {
        let part = reqwest::multipart::Part::bytes(content)
            .file_name(file_name.to_owned())
            .mime_str("text/plain")
            .context(HttpSnafu)?;
        let form = reqwest::multipart::Form::new().part("file", part);

        let resp = self
            .http
            .post(self.url("/v1/files"))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context(HttpSnafu)?;
        let created: CreatedFile = decode(resp).await?;

        let attach = AttachRequest {
            file_id: &created.id,
            external_id,
            overwrite: true,
            metadata,
        };
        let resp = self
            .http
            .post(self.url(&format!("/v1/stores/{store}/files")))
            .bearer_auth(&self.api_key)
            .json(&attach)
            .send()
            .await
            .context(HttpSnafu)?;
        expect_ok(resp).await
    }

    /// Delete one file by external id (or store file id).
    ///
    /// # Errors
    /// Returns an error if the delete request fails.
    pub async fn delete_file(&self, store: &str, external_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(self.url(&format!("/v1/stores/{store}/files/{external_id}")))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context(HttpSnafu)?;
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
    ) -> Result<Vec<Chunk>> {
        let request = SearchRequest {
            query,
            store_identifiers: stores,
            top_k,
            search_options: options,
        };
        let resp = self
            .http
            .post(self.url("/v1/stores/search"))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context(HttpSnafu)?;
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
    ) -> Result<AnswerResponse> {
        let request = SearchRequest {
            query,
            store_identifiers: stores,
            top_k,
            search_options: options,
        };
        let resp = self
            .http
            .post(self.url("/v1/stores/question-answering"))
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .await
            .context(HttpSnafu)?;
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
        let resp = self
            .http
            .get(self.url(&format!("/v1/stores/{store}")))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context(HttpSnafu)?;
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

#[derive(serde::Serialize)]
struct ListRequest<'a> {
    limit: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<&'a str>,
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
    use super::{Chunk, RawChunk};

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
}
