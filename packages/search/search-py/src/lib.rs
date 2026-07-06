//! Python bindings for `search-core`.
//!
//! Three thin async entry points, [`semantic`], [`grep`], and [`recent`], that
//! query the shared corpus store the `indexer` populates (code plus
//! agent/shell history) and return each hit as a plain Python dict. This
//! binding never indexes: it is a read-only query surface, so importing
//! `search` from the MCP session searches the fleet corpus and never uploads
//! the local checkout. Scope a query server-side with
//! `source`/`not_source`/`repo`/`user`/`host`/`project` and a time window
//! (`since`/`until`); with no selector it searches the whole corpus.
//!
//! All query, dedup, and filter logic lives in the core crate; this module only
//! converts at the boundary.
//!
//! The returned awaitable is a native asyncio coroutine bridged through
//! pyo3-async-runtimes, so callers `await` it on their own event loop.

use file_search::{EphemeralSearch, SearchIndex, SearchIndexReader};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use search_core::{
    CodeScope, DEFAULT_STORE, DisplayHit, Filter, FilterSpec, GrepOptions, GrepTargets,
    KNOWN_SOURCE_TAGS, Manifest, MixedbreadStore, RenderMode, Rerank, SearchOptions, Source,
    build_filter, parse_time_spec,
};

/// A `since=`/`until=` argument: epoch seconds as an int, or a string holding
/// either epoch seconds or a relative span (`"30m"`, `"24h"`, `"7d"`, `"2w"`).
#[derive(FromPyObject)]
enum TimeSpec {
    /// Epoch seconds.
    Int(i64),
    /// Epoch seconds or a relative span, parsed by `search_core::parse_time_spec`.
    Str(String),
}

/// Resolve an optional time argument to epoch seconds against the current
/// wall clock; a bad string is a `ValueError`.
fn resolve_time(value: Option<TimeSpec>) -> PyResult<Option<i64>> {
    let Some(value) = value else { return Ok(None) };
    match value {
        TimeSpec::Int(epoch) => Ok(Some(epoch)),
        TimeSpec::Str(text) => parse_time_spec(&text, epoch_now())
            .map(Some)
            .map_err(|error| PyValueError::new_err(error.to_string())),
    }
}

/// The current wall clock as epoch seconds, the reference point for relative
/// `since`/`until` spans.
fn epoch_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Clamp explicitly: a wall clock past i64::MAX epoch seconds is not a real
    // input, and the clamp makes the conversion below infallible.
    let capped = secs.min(u64::try_from(i64::MAX).expect("i64::MAX is positive"));
    i64::try_from(capped).expect("capped at i64::MAX")
}

/// The projection mode for a `compact=` flag.
const fn render_mode(compact: bool) -> RenderMode {
    if compact {
        RenderMode::Compact
    } else {
        RenderMode::Full
    }
}

/// Run a natural-language semantic search over the shared corpus store.
///
/// Returns an awaitable resolving to a list of dicts, one per hit, each with
/// keys `path`, `score`, `start_line`, `num_lines`, `text`, and `source`, plus
/// the provenance keys `timestamp` (epoch seconds), `user`, `host`,
/// `session_id`, `external_id`, `url`, `repo`, and `project` when the record
/// carries them. The scope selectors (including `since`/`until`, epoch seconds
/// or relative spans like `"24h"`/`"7d"`) narrow the query server-side; `web`
/// mixes in the hosted web-search store. No local checkout is read or indexed.
///
/// `compact=True` collapses repeated chunks of one document (keeping the
/// best-scoring) and caps each snippet at 400 characters — a default top_k=10
/// full response measured ~20k tokens, compact ~10x less — full text stays one
/// call away with `compact=False`.
///
/// `agentic` defaults to `False` on every surface (this binding, the `search`
/// CLI, MCP): it is a pass-through to the backend's multi-round search, which
/// measured 10-23s per query (vs 3-6s reranked single-shot) at ~5x the
/// per-query price, and may return fewer than `top_k` hits (it gates results
/// on its own judged relevance, on a different score scale than the
/// reranker). Reach for `agentic=True` only when recall matters more than
/// latency.
///
/// `rerank` toggles the second-stage reranker (on by default). `reranker` names
/// the model: when unset the listwise reranker is used, which reads the
/// candidate set as a whole and lifts ranking quality over the pointwise
/// default.
#[pyfunction]
#[pyo3(signature = (
    query,
    top_k = 10,
    store = None,
    base_url = None,
    rerank = true,
    web = false,
    source = None,
    not_source = None,
    repo = None,
    user = None,
    host = None,
    project = None,
    agentic = false,
    // Trailing optionals so existing positional callers (…, rerank, web, …)
    // keep their slots; inserting one mid-signature would rebind their
    // arguments.
    reranker = None,
    since = None,
    until = None,
    compact = false,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the query + scope surface"
)]
fn semantic(
    py: Python<'_>,
    query: String,
    top_k: usize,
    store: Option<String>,
    base_url: Option<String>,
    rerank: bool,
    web: bool,
    source: Option<Vec<String>>,
    not_source: Option<Vec<String>>,
    repo: Option<String>,
    user: Option<Vec<String>>,
    host: Option<Vec<String>>,
    project: Option<Vec<String>>,
    agentic: bool,
    reranker: Option<String>,
    since: Option<TimeSpec>,
    until: Option<TimeSpec>,
    compact: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let filter = scope_filter(source, not_source, repo, user, host, project, since, until)?;
    // `rerank=False` disables reranking; otherwise a named model wins, falling
    // back to the listwise reranker so the interactive MCP surface gets the best
    // ordering by default.
    let rerank = match (rerank, reranker) {
        (false, _) => Rerank::off(),
        (true, Some(model)) => Rerank::model(model),
        (true, None) => Rerank::listwise(),
    };
    let options = SearchOptions {
        rerank,
        agentic: search_core::Agentic::Toggle(agentic),
        ..SearchOptions::default()
    };
    // Keep every value the borrowed `search_core::semantic` call reads owned in
    // one frame, so the future handed to `future_into_py` stays `'static`.
    let args = SearchArgs {
        query,
        top_k,
        store_name,
        base,
        include_web: web,
        options,
        filter,
        mode: render_mode(compact),
    };
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let hits = run_search(args)
            .await
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

        Python::attach(|py| {
            let out = pyo3::types::PyList::empty(py);
            for hit in &hits {
                out.append(hit_to_dict(py, hit)?)?;
            }
            Ok(out.unbind())
        })
    })
}

/// Run a regular-expression grep over the same corpus chunks as [`semantic`].
///
/// Returns an awaitable resolving to a list of dicts with the same keys as
/// [`semantic`]. `case_sensitive` toggles case folding; grep never queries the
/// web store. No local checkout is read or indexed.
#[pyfunction]
#[pyo3(signature = (
    pattern,
    top_k = 10,
    store = None,
    base_url = None,
    case_sensitive = false,
    source = None,
    not_source = None,
    repo = None,
    user = None,
    host = None,
    project = None,
    since = None,
    until = None,
    compact = false,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the grep + scope surface"
)]
fn grep(
    py: Python<'_>,
    pattern: String,
    top_k: usize,
    store: Option<String>,
    base_url: Option<String>,
    case_sensitive: bool,
    source: Option<Vec<String>>,
    not_source: Option<Vec<String>>,
    repo: Option<String>,
    user: Option<Vec<String>>,
    host: Option<Vec<String>>,
    project: Option<Vec<String>>,
    since: Option<TimeSpec>,
    until: Option<TimeSpec>,
    compact: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let filter = scope_filter(source, not_source, repo, user, host, project, since, until)?;
    let options = GrepOptions {
        case_sensitive,
        targets: GrepTargets::Text,
    };
    let args = GrepArgs {
        pattern,
        top_k,
        store_name,
        base,
        options,
        filter,
        mode: render_mode(compact),
    };
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let hits = run_grep(args)
            .await
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

        Python::attach(|py| {
            let out = pyo3::types::PyList::empty(py);
            for hit in &hits {
                out.append(hit_to_dict(py, hit)?)?;
            }
            Ok(out.unbind())
        })
    })
}

/// List the newest corpus records (descending timestamp) matching the scope —
/// a deterministic "what happened lately" feed with no semantic scoring or
/// reranking, backed by the store's metadata-only chunk listing.
///
/// Returns an awaitable resolving to the same hit dicts as [`semantic`],
/// newest first. The `score` value is the API's placeholder, not relevance.
/// `compact` defaults to `True` here: a recency feed is usually scanned, not
/// read, and Claude-history bodies are large; pass `compact=False` for full
/// text.
///
///     # my shell commands of the last six hours, newest first
///     rows = await search.recent(source=["shell"], user=["andrew"], since="6h")
#[pyfunction]
#[pyo3(signature = (
    top_k = 20,
    store = None,
    base_url = None,
    source = None,
    not_source = None,
    repo = None,
    user = None,
    host = None,
    project = None,
    since = None,
    until = None,
    compact = true,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the scope surface"
)]
fn recent(
    py: Python<'_>,
    top_k: usize,
    store: Option<String>,
    base_url: Option<String>,
    source: Option<Vec<String>>,
    not_source: Option<Vec<String>>,
    repo: Option<String>,
    user: Option<Vec<String>>,
    host: Option<Vec<String>>,
    project: Option<Vec<String>>,
    since: Option<TimeSpec>,
    until: Option<TimeSpec>,
    compact: bool,
) -> PyResult<Bound<'_, PyAny>> {
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let filter = scope_filter(source, not_source, repo, user, host, project, since, until)?;
    let mode = render_mode(compact);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let hits = async {
            let store = MixedbreadStore::from_login(base).await?;
            search_core::recent(&store, &store_name, top_k, filter.as_ref(), mode).await
        }
        .await
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

        Python::attach(|py| {
            let out = pyo3::types::PyList::empty(py);
            for hit in &hits {
                out.append(hit_to_dict(py, hit)?)?;
            }
            Ok(out.unbind())
        })
    })
}

/// Build the server-side metadata filter from the scope selectors, or `None`
/// when nothing is constrained. Shared by [`semantic`], [`grep`], and
/// [`recent`] so the mapping matches the `search` CLI exactly (one builder in
/// `search-core`).
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the scope surface"
)]
fn scope_filter(
    sources: Option<Vec<String>>,
    not_sources: Option<Vec<String>>,
    repo: Option<String>,
    users: Option<Vec<String>>,
    hosts: Option<Vec<String>>,
    projects: Option<Vec<String>>,
    since: Option<TimeSpec>,
    until: Option<TimeSpec>,
) -> PyResult<Option<Filter>> {
    let spec = FilterSpec {
        sources: parse_sources(sources)?,
        exclude_sources: parse_sources(not_sources)?,
        repo: repo.filter(|value| !value.is_empty()),
        users: split_csv(users),
        hosts: split_csv(hosts),
        projects: split_csv(projects),
        since: resolve_time(since)?,
        until: resolve_time(until)?,
    };
    Ok(build_filter(&spec))
}

/// Parse source tags, accepting repeated and comma-joined values
/// (`["code", "slack,linear"]`). An unknown tag is a `ValueError` listing the
/// valid tags: the store silently accepts any tag and returns zero hits, which
/// is indistinguishable from an empty corpus, so a typo must fail loudly here.
fn parse_sources(values: Option<Vec<String>>) -> PyResult<Vec<Source>> {
    let mut out = Vec::new();
    for value in values.unwrap_or_default() {
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if !KNOWN_SOURCE_TAGS.contains(&part) {
                return Err(PyValueError::new_err(format!(
                    "unknown source {part:?}; valid sources: {}",
                    KNOWN_SOURCE_TAGS.join(", ")
                )));
            }
            out.push(Source::new(part));
        }
    }
    Ok(out)
}

/// Flatten repeated, comma-joined string selectors (`["a,b", "c"]`) into one
/// list, trimming whitespace and dropping blanks. Mirrors the CLI's `split_csv`.
fn split_csv(values: Option<Vec<String>>) -> Vec<String> {
    values
        .unwrap_or_default()
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Owned inputs for one search, so [`run_search`] can build the borrowed query
/// from values it owns for the whole call.
struct SearchArgs {
    query: String,
    top_k: usize,
    store_name: String,
    base: String,
    include_web: bool,
    options: SearchOptions,
    filter: Option<Filter>,
    mode: RenderMode,
}

/// Query the corpus store and return owned hits. The manifest is empty (this
/// binding never reads a checkout), so code is scoped entirely server-side.
async fn run_search(args: SearchArgs) -> search_core::Result<Vec<DisplayHit>> {
    let store = MixedbreadStore::from_login(args.base.clone()).await?;
    let manifest = Manifest::default();
    search_core::semantic(
        &store,
        &args.store_name,
        &manifest,
        &args.query,
        args.top_k,
        args.options,
        args.include_web,
        args.filter.as_ref(),
        CodeScope::ServerFiltered,
        args.mode,
    )
    .await
}

/// Owned inputs for one grep, so [`run_grep`] can build the borrowed query from
/// values it owns for the whole call.
struct GrepArgs {
    pattern: String,
    top_k: usize,
    store_name: String,
    base: String,
    options: GrepOptions,
    filter: Option<Filter>,
    mode: RenderMode,
}

/// Grep the corpus store and return owned hits. Like [`run_search`], the empty
/// manifest leaves code scoping to the server-side filter.
async fn run_grep(args: GrepArgs) -> search_core::Result<Vec<DisplayHit>> {
    let store = MixedbreadStore::from_login(args.base.clone()).await?;
    let manifest = Manifest::default();
    search_core::grep(
        &store,
        &args.store_name,
        &manifest,
        &args.pattern,
        args.top_k,
        args.options,
        args.filter.as_ref(),
        CodeScope::ServerFiltered,
        args.mode,
    )
    .await
}

/// Convert one [`DisplayHit`] into the public Python dict shape. The base keys
/// are always present; the provenance keys are set only when the record
/// carries them, so sources that never write them add no key (and no tokens).
fn hit_to_dict<'py>(py: Python<'py>, hit: &DisplayHit) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("path", &hit.label)?;
    dict.set_item("score", hit.score)?;
    dict.set_item("start_line", hit.start_line)?;
    dict.set_item("num_lines", hit.num_lines)?;
    dict.set_item("text", &hit.text)?;
    dict.set_item("source", hit.source.as_str())?;
    if let Some(timestamp) = hit.timestamp {
        dict.set_item("timestamp", timestamp)?;
    }
    let optional = [
        ("user", &hit.user),
        ("host", &hit.host),
        ("session_id", &hit.session_id),
        ("external_id", &hit.external_id),
        ("url", &hit.url),
        ("repo", &hit.repo),
        ("project", &hit.project),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            dict.set_item(key, value)?;
        }
    }
    Ok(dict)
}

/// Rerank a batch of texts against a query by BM25, in memory.
///
/// Builds a one-shot Tantivy index over `texts` and scores them against
/// `query`, returning up to `limit` hits (default: every text) ranked best
/// first. Each hit is a dict with `index` (the position in the input list),
/// `score` (BM25), and `text` (the matched string). Texts that match nothing
/// are omitted. Unlike [`semantic`]/[`grep`]/[`recent`], this does no network
/// I/O and reads no corpus, so it is a plain synchronous function: it returns
/// the list directly, not an awaitable.
///
/// Tokenization is inherited from `file_search` (the shared `code-tokenizer`
/// stemmed tokenizer), so identifiers split on camelCase, snake_case,
/// kebab-case, and whitespace before stemming: `"widget factory"` matches both
/// `makeWidgetFactory` and `make_widget_factory`.
///
///     hits = search.bm25_rerank("retry backoff", candidate_texts, limit=5)
#[pyfunction]
#[pyo3(signature = (query, texts, limit = None))]
fn bm25_rerank(
    py: Python<'_>,
    query: &str,
    texts: Vec<String>,
    limit: Option<usize>,
) -> PyResult<Py<PyAny>> {
    // Default to ranking the whole batch; the search never returns more hits
    // than documents, so an oversized limit is harmless.
    let limit = limit.unwrap_or(texts.len());
    // The indexing + scoring is pure Rust CPU work; detach from the GIL so a
    // caller's other threads (e.g. an asyncio loop) keep running.
    let ranked = py
        .detach(|| rerank(query, texts, limit))
        .map_err(PyRuntimeError::new_err)?;

    let out = pyo3::types::PyList::empty(py);
    for hit in ranked {
        let dict = PyDict::new(py);
        dict.set_item("index", hit.index)?;
        dict.set_item("score", hit.score)?;
        dict.set_item("text", hit.text)?;
        out.append(dict)?;
    }
    Ok(out.unbind().into_any())
}

/// Build (or update) an on-disk BM25 index over the files under `path`.
///
/// Walks `path` and indexes every text-shaped file into the Tantivy index at
/// `index_dir`, creating it if absent — the same pipeline as the `file-search
/// index` CLI subcommand, so it inherits the shared `code-tokenizer`. Honors
/// `.gitignore` unless `no_gitignore=True`. Synchronous (local disk work, no
/// network). Returns a dict with `files_indexed`, `files_skipped`, and
/// `errors` (a list of `[path, message]` pairs for files that could not be
/// read or parsed; these do not abort the walk).
#[pyfunction]
#[pyo3(signature = (path, index_dir, no_gitignore = false))]
fn bm25_index(
    py: Python<'_>,
    path: &str,
    index_dir: &str,
    no_gitignore: bool,
) -> PyResult<Py<PyAny>> {
    // The walk + Tantivy indexing is blocking disk/CPU work; detach from the
    // GIL so a caller's other threads (e.g. an asyncio loop) keep running.
    let stats = py
        .detach(|| {
            let mut index = SearchIndex::open_or_create(index_dir)?;
            index.index_directory(std::path::Path::new(path), !no_gitignore)
        })
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

    let dict = PyDict::new(py);
    dict.set_item("files_indexed", stats.files_indexed)?;
    dict.set_item("files_skipped", stats.files_skipped)?;
    let errors = pyo3::types::PyList::empty(py);
    for (path, message) in &stats.errors {
        let pair = pyo3::types::PyList::empty(py);
        pair.append(path.to_string_lossy())?;
        pair.append(message)?;
        errors.append(pair)?;
    }
    dict.set_item("errors", errors)?;
    Ok(dict.unbind().into_any())
}

/// Search an on-disk BM25 index built by [`bm25_index`].
///
/// Opens the index at `index_dir` read-only (no writer lock, so it can run
/// alongside an indexing pass) and returns up to `limit` hits matching `query`
/// in Tantivy query syntax, ranked best first — the `file-search search`
/// subcommand. When `filter` is set, only files under that directory match.
/// Synchronous. Each hit is a dict with `path`, `score`, `snippet`, and
/// `chunk_offset`.
#[pyfunction]
#[pyo3(signature = (query, index_dir, limit = 10, filter = None))]
fn bm25_search(
    py: Python<'_>,
    query: &str,
    index_dir: &str,
    limit: usize,
    filter: Option<&str>,
) -> PyResult<Py<PyAny>> {
    // Opening and querying the index is blocking disk/CPU work; detach from
    // the GIL so a caller's other threads (e.g. an asyncio loop) keep running.
    let hits = py
        .detach(|| {
            let reader = SearchIndexReader::open(index_dir)?;
            reader.search(query, limit, filter.map(std::path::Path::new))
        })
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

    let out = pyo3::types::PyList::empty(py);
    for hit in hits {
        let dict = PyDict::new(py);
        dict.set_item("path", hit.path)?;
        dict.set_item("score", hit.score)?;
        dict.set_item("snippet", hit.snippet)?;
        dict.set_item("chunk_offset", hit.chunk_offset)?;
        out.append(dict)?;
    }
    Ok(out.unbind().into_any())
}

/// One reranked text: its position in the input batch, BM25 score, and body.
struct Reranked {
    index: usize,
    score: f32,
    text: String,
}

/// Rank `texts` against `query` through `file_search`'s [`EphemeralSearch`],
/// which registers the shared `code-tokenizer` on its in-memory index. Kept
/// free of `pyo3` types so `#[cfg(test)]` can exercise the tokenizer
/// inheritance without a Python interpreter.
fn rerank(query: &str, texts: Vec<String>, limit: usize) -> Result<Vec<Reranked>, String> {
    let search = EphemeralSearch::from_texts(texts.iter().cloned()).map_err(|e| e.to_string())?;
    let ranked = search.search(query, limit).map_err(|e| e.to_string())?;
    Ok(ranked
        .into_iter()
        .map(|hit| Reranked {
            index: hit.id,
            score: hit.score,
            // `hit.id` indexes back into the batch we just built the index from.
            text: texts[hit.id].clone(),
        })
        .collect())
}

#[pymodule]
fn _search(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(semantic, module)?)?;
    module.add_function(wrap_pyfunction!(grep, module)?)?;
    module.add_function(wrap_pyfunction!(recent, module)?)?;
    module.add_function(wrap_pyfunction!(bm25_rerank, module)?)?;
    module.add_function(wrap_pyfunction!(bm25_index, module)?)?;
    module.add_function(wrap_pyfunction!(bm25_search, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rerank;

    /// The binding routes through `file_search::EphemeralSearch`, so it inherits
    /// the shared `code-tokenizer`: a query of two plain words matches an
    /// identifier written in camelCase and one in snake_case. If a future change
    /// bypassed `file_search` and built a raw Tantivy schema with the default
    /// tokenizer, neither identifier would split and this would return no hits.
    #[test]
    fn rerank_inherits_code_tokenizer() {
        let texts = vec![
            "fn makeWidgetFactory() {}".to_owned(),
            "def make_widget_factory(): pass".to_owned(),
            "the quick brown fox".to_owned(),
        ];
        let hits = rerank("widget factory", texts, 10).expect("rerank succeeds");
        let matched: Vec<usize> = hits.iter().map(|h| h.index).collect();
        assert!(
            matched.contains(&0),
            "camelCase makeWidgetFactory should match; got {matched:?}"
        );
        assert!(
            matched.contains(&1),
            "snake_case make_widget_factory should match; got {matched:?}"
        );
        assert!(
            !matched.contains(&2),
            "unrelated text should not match; got {matched:?}"
        );
    }

    /// `bm25_rerank(query, [])` defaults the limit to the batch size — zero.
    /// Tantivy's `TopDocs::with_limit(0)` panics, which pyo3 would re-raise as
    /// a `PanicException`; `file_search` guards it, so this must return an
    /// empty list instead.
    #[test]
    fn rerank_empty_batch_returns_no_hits() {
        let hits = rerank("anything", Vec::new(), 0).expect("empty batch must not panic");
        assert!(hits.is_empty(), "empty batch should yield no hits");
    }
}
