# mixedbread

`packages/mixedbread` (crate `mixedbread`) is a minimal async Rust client for the
[Mixedbread](https://www.mixedbread.com) vector store API. It owns HTTP and JSON
shapes only, no domain logic, so it backs the search stack
([`search-core`](../search-core/overview.md)'s `MixedbreadStore`, the
[sinks](../sink/overview.md), the [indexer](../indexer/overview.md)) and the
[`polars-mixedbread`](../polars-mixedbread/overview.md) binding alike. Library
crate, no flake output; `passthruTests` runs its (wire-shape) tests in CI.

## Endpoints (`src/lib.rs`)

[`Client`] (`src/lib.rs:592`) covers: store create/get (`/v1/stores`); the
two-step file upload (`/v1/files` then `/v1/stores/{store}/files`), file listing,
per-file status, and delete; search (`/v1/stores/search`), regex grep
(`/v1/stores/grep`), metadata-only chunk listing (`/v1/stores/list-chunks`),
question-answering (`/v1/stores/question-answering`), query enhancement
(`/v1/stores/queries/enhance`), metadata facets (`/v1/stores/metadata-facets`),
the standalone reranker (`/v1/rerank`-style `rerank`), and the per-store event
histogram (`src/lib.rs:1-12`). The methods (all `pub async fn`):
`ensure_store` (`:698`), `list_files` (`:725`), `upload_file` (`:767`),
`file_status` (`:817`), `delete_file` (`:834`), `search` (`:848`), `grep`
(`:885`), `list_chunks` (`:925`), `ask` (`:957`), `enhance_query` (`:1001`),
`rerank` (`:1032`), `store_status` (`:1066`), `metadata_facets` (`:1092`),
`events_histogram` (`:1126`).

## Construction and auth

`Client::new(base_url, api_key)` (`src/lib.rs:603`), `from_env` (the
`MXBAI_API_KEY` env var only, `src/lib.rs:617`), or `from_login`
(`src/lib.rs:632`): the production path, which resolves the credential via
`auth::resolve_token` (`src/auth.rs:39`), preferring `MXBAI_API_KEY` and
otherwise reading the OAuth token `mgrep login` wrote at `~/.mgrep/token.json`
and exchanging it for a short-lived API JWT at the platform auth endpoint
(`PLATFORM_URL`, the same exchange the `mgrep` CLI does, `src/auth.rs:18`).
`DEFAULT_BASE_URL` is `https://api.mixedbread.com` (`src/lib.rs:30`);
`API_KEY_ENV` is `MXBAI_API_KEY` (`src/lib.rs:40`).

## Reliability

Every request is bounded: a 30s connect timeout and a 2-minute request timeout
(`bounded_http_builder`, `src/lib.rs:95`), so a shed stream surfaces as a
retryable transport error rather than an unbounded await (this once wedged the
leader's reconcile bootstrap for over an hour, `src/lib.rs:81-89`). Retryable
statuses (429 and 5xx) are retried up to `MAX_RETRIES` (6) honoring the server's
`Retry-After`, otherwise equal-jitter exponential backoff from `BACKOFF_BASE`
(500ms) capped at `BACKOFF_CAP` (30s) (`src/lib.rs:60-75`). Listing pages at
`LIST_PAGE_SIZE` (100, the API max) following a cursor (`src/lib.rs:32-37`).
External ids are percent-encoded into URL paths (a `/` in `github:org/repo` would
otherwise split the route) via the `PATH_SEGMENT` set (`src/lib.rs:42-58`).

## Types and filters

Option/result types: [`SearchOptions`] (`src/lib.rs:350`), [`QaOptions`]
(`src/lib.rs:387`), [`Rerank`] (`src/lib.rs:197`, `DEFAULT_RERANK_MODEL =
mixedbread-ai/mxbai-rerank-v3-listwise`, `src/lib.rs:186`), [`Agentic`] /
`AgenticConfig` (`src/lib.rs:251`), [`Chunk`] (`src/lib.rs:450`, a search hit),
[`StoredFile`] (`src/lib.rs:433`), [`FileStatus`] (`src/lib.rs:562`),
[`FacetLimits`] / `FACETS_MAX_FILES` (100000, `src/lib.rs:501`), `EventType`,
`StoreStatus`, `HistogramBucket`.

The `filter` module (`src/filter.rs`) is the metadata filter DSL: a recursive
[`Filter`] of leaf [`Condition`]s (`{key, operator, value}`) and [`Group`]s
(`all`/`any`/`none`), mirroring the API's wire shape across search, grep,
question-answering, and file listing (`src/filter.rs:1-9`). [`Operator`]
(`src/filter.rs:29`) serializes to the exact API tokens (`eq`, `not_eq`, `gt`,
`gte`, `in`, `like`, `starts_with`, `regex`, ...). It is `Deserialize` too, so a
caller passing a JSON-built filter (the polars binding) parses it into the typed
DSL rather than an unchecked blob. The `enhance` module
(`src/enhance.rs`) carries `EnhancedQuery`/`FilterMode`/`SortDirection` for the
query-enhance endpoint. `search-core` re-exports these types so its consumers
depend only on `search-core`.

[`Client`]: #endpoints-srclibrs
[`SearchOptions`]: #types-and-filters
[`QaOptions`]: #types-and-filters
[`Rerank`]: #types-and-filters
[`Agentic`]: #types-and-filters
[`Chunk`]: #types-and-filters
[`StoredFile`]: #types-and-filters
[`FileStatus`]: #types-and-filters
[`FacetLimits`]: #types-and-filters
[`Filter`]: #types-and-filters
[`Condition`]: #types-and-filters
[`Group`]: #types-and-filters
[`Operator`]: #types-and-filters
