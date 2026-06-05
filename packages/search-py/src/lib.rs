//! Python bindings for `search-core`.
//!
//! Two thin async entry points, [`semantic`] and [`grep`], that query the shared
//! corpus store the `indexer` populates (code plus agent/shell history) and
//! return each hit as a plain Python dict. This binding never indexes: it is a
//! read-only query surface, so importing `search` from the MCP session searches
//! the fleet corpus and never uploads the local checkout. Scope a query
//! server-side with `source`/`not_source`/`repo`/`user`/`host`/`project`; with
//! no selector it searches the whole corpus.
//!
//! All query, dedup, and filter logic lives in the core crate; this module only
//! converts at the boundary.
//!
//! The returned awaitable is a native asyncio coroutine bridged through
//! pyo3-async-runtimes, so callers `await` it on their own event loop.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use search_core::{
    CodeScope, DEFAULT_STORE, DisplayHit, Filter, FilterSpec, GrepOptions, GrepTargets, Manifest,
    MixedbreadStore, Rerank, SearchOptions, Source, build_filter,
};

/// Run a natural-language semantic search over the shared corpus store.
///
/// Returns an awaitable resolving to a list of dicts, one per hit, each with
/// keys `path`, `score`, `start_line`, `num_lines`, `text`, and `source`. The
/// scope selectors narrow the query server-side; `web` mixes in the hosted
/// web-search store. No local checkout is read or indexed.
///
/// `agentic` defaults to `true`: the MCP `search` surface is interactive and
/// recall matters more than latency there, so letting the backend plan and run
/// multiple searches gives better results out of the box. Pass `agentic=False`
/// for a single-shot query when speed beats recall. (The `search` CLI keeps it
/// off by default for scripted, low-latency use.)
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
    agentic = true,
    // Trailing optional so existing positional callers (…, rerank, web, …) keep
    // their slots; inserting it mid-signature would rebind their arguments.
    reranker = None,
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
) -> PyResult<Bound<'_, PyAny>> {
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let filter = scope_filter(source, not_source, repo, user, host, project)?;
    // `rerank=False` disables reranking; otherwise a named model wins, falling
    // back to the listwise reranker so the interactive MCP surface gets the best
    // ordering by default.
    let rerank = match (rerank, reranker) {
        (false, _) => Rerank::off(),
        (true, Some(model)) => Rerank::model(model),
        (true, None) => Rerank::listwise(),
    };
    let options = SearchOptions { rerank, agentic };
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
) -> PyResult<Bound<'_, PyAny>> {
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let filter = scope_filter(source, not_source, repo, user, host, project)?;
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

/// Build the server-side metadata filter from the scope selectors, or `None`
/// when nothing is constrained. Shared by [`semantic`] and [`grep`] so the
/// mapping matches the `search` CLI exactly (one builder in `search-core`).
fn scope_filter(
    sources: Option<Vec<String>>,
    not_sources: Option<Vec<String>>,
    repo: Option<String>,
    users: Option<Vec<String>>,
    hosts: Option<Vec<String>>,
    projects: Option<Vec<String>>,
) -> PyResult<Option<Filter>> {
    let spec = FilterSpec {
        sources: parse_sources(sources)?,
        exclude_sources: parse_sources(not_sources)?,
        repo: repo.filter(|value| !value.is_empty()),
        users: split_csv(users),
        hosts: split_csv(hosts),
        projects: split_csv(projects),
    };
    Ok(build_filter(&spec))
}

/// Parse source tags, accepting repeated and comma-joined values
/// (`["code", "slack,linear"]`). An unknown tag is a `ValueError`.
fn parse_sources(values: Option<Vec<String>>) -> PyResult<Vec<Source>> {
    let mut out = Vec::new();
    for value in values.unwrap_or_default() {
        for part in value.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let source = part.parse::<Source>().map_err(|error| {
                PyValueError::new_err(format!("invalid source {part:?}: {error}"))
            })?;
            out.push(source);
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
    )
    .await
}

/// Convert one [`DisplayHit`] into the public Python dict shape.
fn hit_to_dict<'py>(py: Python<'py>, hit: &DisplayHit) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("path", &hit.label)?;
    dict.set_item("score", hit.score)?;
    dict.set_item("start_line", hit.start_line)?;
    dict.set_item("num_lines", hit.num_lines)?;
    dict.set_item("text", &hit.text)?;
    dict.set_item("source", hit.source.as_str())?;
    Ok(dict)
}

#[pymodule]
fn _search(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(semantic, module)?)?;
    module.add_function(wrap_pyfunction!(grep, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
