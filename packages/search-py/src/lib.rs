//! Python bindings for `search-core`.
//!
//! Two thin async entry points, [`semantic`] and [`grep`], which marshal Python
//! arguments into a [`search_core::Query`], run the index-then-query
//! pipeline, and return each hit as a plain Python dict. All indexing, dedup,
//! and query logic lives in the core crate; this module only converts at the
//! boundary.
//!
//! The returned awaitable is a native asyncio coroutine bridged through
//! pyo3-async-runtimes, so callers `await` it on their own event loop.

use std::path::PathBuf;
use std::time::Duration;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use search_core::{
    CodeScope, Config, DEFAULT_STORE, DisplayHit, GrepOptions, GrepTargets, MixedbreadStore, Query,
    SearchOptions,
};

/// How long to wait for newly uploaded files to embed before querying anyway.
/// Matches the CLI's indexing wait so a first run on a fresh tree behaves the
/// same through either surface.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Index the checkout at `path` (unless `no_sync`) and run a natural-language
/// semantic search over it.
///
/// Returns an awaitable resolving to a list of dicts, one per hit, each with
/// keys `path`, `score`, `start_line`, `num_lines`, `text`, and `is_web`.
#[pyfunction]
#[pyo3(signature = (
    query,
    path,
    top_k = 10,
    store = None,
    base_url = None,
    no_sync = false,
    rerank = true,
    web = false,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the Query surface"
)]
#[allow(
    clippy::fn_params_excessive_bools,
    reason = "each flag is a distinct independent search knob"
)]
fn semantic(
    py: Python<'_>,
    query: String,
    path: String,
    top_k: usize,
    store: Option<String>,
    base_url: Option<String>,
    no_sync: bool,
    rerank: bool,
    web: bool,
) -> PyResult<Bound<'_, PyAny>> {
    // Keep the borrow-heavy pipeline in its own `async fn` returning owned
    // `DisplayHit`s, so the future handed to `future_into_py` is a clean
    // `'static` producer of owned data. Inlining the borrows of `Query` into the
    // `future_into_py` block makes the compiler fail to unify the higher-ranked
    // lifetimes in `index_and_semantic`'s generic bounds.
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let options = SearchOptions {
        rerank,
        agentic: false,
    };
    let args = SearchArgs {
        query,
        path,
        top_k,
        store_name,
        base,
        sync: !no_sync,
        include_web: web,
        options,
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

/// Index the checkout at `path` (unless `no_sync`) and run a regular-expression
/// grep over the same indexed chunks as [`semantic`].
///
/// Returns an awaitable resolving to a list of dicts with the same keys as
/// [`semantic`]. `case_sensitive` toggles case folding; grep never queries the
/// web store.
#[pyfunction]
#[pyo3(signature = (
    pattern,
    path,
    top_k = 10,
    store = None,
    base_url = None,
    no_sync = false,
    case_sensitive = false,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin 1:1 mirror of the grep Query surface"
)]
fn grep(
    py: Python<'_>,
    pattern: String,
    path: String,
    top_k: usize,
    store: Option<String>,
    base_url: Option<String>,
    no_sync: bool,
    case_sensitive: bool,
) -> PyResult<Bound<'_, PyAny>> {
    // Mirror `semantic`: keep owned inputs in a dedicated frame so the future is
    // `'static` over the borrowed `Query` the pipeline builds.
    let store_name = store.unwrap_or_else(|| DEFAULT_STORE.to_owned());
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let options = GrepOptions {
        case_sensitive,
        targets: GrepTargets::Text,
    };
    let args = GrepArgs {
        pattern,
        path,
        top_k,
        store_name,
        base,
        sync: !no_sync,
        options,
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

/// Owned inputs for one search, so [`run_search`] can build a borrowed
/// [`Query`] from values it owns for the whole call.
struct SearchArgs {
    query: String,
    path: String,
    top_k: usize,
    store_name: String,
    base: String,
    sync: bool,
    include_web: bool,
    options: SearchOptions,
}

/// Run the index-then-search pipeline and return owned hits. Keeping every
/// value `Query` borrows owned in this frame is what lets the caller's future
/// stay `'static`.
async fn run_search(args: SearchArgs) -> search_core::Result<Vec<DisplayHit>> {
    let root: PathBuf =
        std::fs::canonicalize(&args.path).unwrap_or_else(|_| PathBuf::from(&args.path));
    let store = MixedbreadStore::from_login(args.base.clone()).await?;

    let query = Query {
        root: &root,
        store_name: &args.store_name,
        base_url: &args.base,
        text: &args.query,
        top_k: args.top_k,
        options: args.options,
        sync: args.sync,
        include_web: args.include_web,
        filters: None,
        code_scope: CodeScope::WorktreeExact,
        // The Python/MCP binding searches a checkout (code is always in scope).
        index_code: true,
        index_timeout: INDEX_TIMEOUT,
    };

    search_core::index_and_semantic(&store, &query, &Config::default(), |_, _| {}, |_| {})
        .await
}

/// Owned inputs for one grep, so [`run_grep`] can build a borrowed [`Query`]
/// from values it owns for the whole call.
struct GrepArgs {
    pattern: String,
    path: String,
    top_k: usize,
    store_name: String,
    base: String,
    sync: bool,
    options: GrepOptions,
}

/// Run the index-then-grep pipeline and return owned hits. Keeping every value
/// `Query` borrows owned in this frame is what lets the caller's future stay
/// `'static`.
async fn run_grep(args: GrepArgs) -> search_core::Result<Vec<DisplayHit>> {
    let root: PathBuf =
        std::fs::canonicalize(&args.path).unwrap_or_else(|_| PathBuf::from(&args.path));
    let store = MixedbreadStore::from_login(args.base.clone()).await?;

    // Grep ignores semantic-only knobs (`options`, `include_web`); they are set
    // to inert defaults so the shared `Query` shape stays reusable.
    let query = Query {
        root: &root,
        store_name: &args.store_name,
        base_url: &args.base,
        text: &args.pattern,
        top_k: args.top_k,
        options: SearchOptions {
            rerank: false,
            agentic: false,
        },
        sync: args.sync,
        include_web: false,
        filters: None,
        code_scope: CodeScope::WorktreeExact,
        // The Python/MCP binding searches a checkout (code is always in scope).
        index_code: true,
        index_timeout: INDEX_TIMEOUT,
    };

    search_core::index_and_grep(
        &store,
        &query,
        args.options,
        &Config::default(),
        |_, _| {},
        |_| {},
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
