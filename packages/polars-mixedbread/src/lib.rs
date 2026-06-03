//! The Rust core behind `scan_mixedbread`: run one Mixedbread store search and
//! hand the hits back to Python as plain columnar data.
//!
//! This is deliberately thin. All HTTP, retry, auth, and the metadata filter DSL
//! live in the workspace [`mixedbread`] client; this module only blocks on the
//! async search and converts the resulting [`mixedbread::Chunk`]s into a dict of
//! column-name to list. Everything else, the lazy plumbing, the fixed schema,
//! flattening metadata into columns, predicate pushdown, projection, and the row
//! limit, lives in the Python wrapper (`polars_mixedbread/__init__.py`), because
//! Polars' IO-plugin interface is Python by design.
//!
//! Polars never appears on the Rust side: the wrapper builds the `DataFrame` with
//! the *runtime* Polars, so there is no Rust/Python Polars version coupling and
//! no `pyo3-polars` Arrow-FFI lockstep to keep in sync (unlike `polars-sftp`,
//! which decodes files in Rust and so must pin both).

use mixedbread::{Client, SearchOptions, filter::Filter};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// Run a Mixedbread store search and return its hits as a dict of column lists.
///
/// `stores` are store identifiers to search; `query` is the natural-language
/// query; `top_k` caps hits. `filters` is the Mixedbread metadata filter as a
/// JSON string (the recursive `{key, operator, value}` / `all`/`any`/`none`
/// shape), pushed server-side; the wrapper builds it from the Polars predicate.
/// Authentication mirrors the `search` surface: `MXBAI_API_KEY`, else the
/// `mgrep login` token.
///
/// The returned dict always has the same six columns: `text`, `score`,
/// `filename`, `start_line`, `num_lines`, `metadata` (the last a JSON string).
/// Projection, the row limit, and flattening metadata into typed columns are the
/// wrapper's job, so this stays a straight search-to-columns conversion.
#[pyfunction]
#[pyo3(signature = (
    stores,
    query,
    top_k = 10,
    base_url = None,
    rerank = true,
    agentic = false,
    score_threshold = None,
    filters = None,
))]
#[allow(
    clippy::too_many_arguments,
    reason = "thin PyO3 binding mirrors scan_mixedbread's search + options surface"
)]
fn search_mixedbread(
    py: Python<'_>,
    stores: Vec<String>,
    query: String,
    top_k: usize,
    base_url: Option<String>,
    rerank: bool,
    agentic: bool,
    score_threshold: Option<f32>,
    filters: Option<String>,
) -> PyResult<Py<PyDict>> {
    if stores.is_empty() {
        return Err(PyValueError::new_err(
            "polars-mixedbread: at least one store identifier is required",
        ));
    }
    let base = base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    // Parse the caller's filter JSON into the typed owner DSL, so a malformed
    // filter is a `ValueError` here rather than a silently-wrong query later.
    let filter = filters
        .map(|json| serde_json::from_str::<Filter>(&json))
        .transpose()
        .map_err(|error| {
            PyValueError::new_err(format!("polars-mixedbread: invalid filters JSON: {error}"))
        })?;
    let options = SearchOptions {
        rerank,
        agentic,
        score_threshold,
        // Always ask for file metadata: the wrapper flattens it into columns, and
        // group-by/filter on those columns is the whole point.
        return_metadata: Some(true),
    };

    // Release the GIL for the blocking network round-trip: a current-thread
    // runtime drives the async client to completion while other Python threads
    // (and Polars' engine) keep running.
    let chunks = py
        .detach(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| format!("build runtime: {error}"))?;
            runtime.block_on(async {
                let client = Client::from_login(base)
                    .await
                    .map_err(|error| format!("authenticate: {error}"))?;
                client
                    .search(&stores, &query, top_k, options, filter.as_ref())
                    .await
                    .map_err(|error| format!("search: {error}"))
            })
        })
        .map_err(|error| PyRuntimeError::new_err(format!("polars-mixedbread: {error}")))?;

    columns_to_dict(py, &chunks)
}

/// Build the `{column: [values...]}` dict with all six source columns.
fn columns_to_dict(py: Python<'_>, chunks: &[mixedbread::Chunk]) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);

    let text: Vec<Option<&str>> = chunks.iter().map(|c| c.text.as_deref()).collect();
    dict.set_item("text", PyList::new(py, text)?)?;

    let score: Vec<f64> = chunks.iter().map(|c| f64::from(c.score)).collect();
    dict.set_item("score", PyList::new(py, score)?)?;

    let filename: Vec<Option<&str>> = chunks.iter().map(|c| c.filename.as_deref()).collect();
    dict.set_item("filename", PyList::new(py, filename)?)?;

    let start_line: Vec<Option<u32>> = chunks.iter().map(|c| c.start_line).collect();
    dict.set_item("start_line", PyList::new(py, start_line)?)?;

    // `Chunk::num_lines` is the API's `end_line - start_line` span, so an N-line
    // chunk reports `N - 1`; expose a line *count* by adding one (the same
    // normalization search-core applies), so a Polars consumer can sum it.
    let num_lines: Vec<Option<u32>> = chunks
        .iter()
        .map(|c| c.num_lines.map(|span| span.saturating_add(1)))
        .collect();
    dict.set_item("num_lines", PyList::new(py, num_lines)?)?;

    // Metadata is arbitrary nested JSON, so it crosses as a JSON string and the
    // wrapper flattens the declared keys into typed columns (and leaves the rest
    // queryable via `.str.json_decode()`), rather than guessing a struct schema
    // that varies per store.
    let metadata: Vec<Option<String>> = chunks
        .iter()
        .map(|c| c.metadata.as_ref().map(ToString::to_string))
        .collect();
    dict.set_item("metadata", PyList::new(py, metadata)?)?;

    Ok(dict.unbind())
}

#[pymodule]
fn _polars_mixedbread(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(search_mixedbread, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
