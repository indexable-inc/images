//! Python bindings for `scipql-core`.
//!
//! Thin sync entry points: [`index`] runs `rust-analyzer scip`, [`facts`]
//! lowers an index to relations, [`query`] runs a Soufflé program and returns
//! its output relations, and [`fix`]/[`rename`] apply edits (returning the
//! unified diff). All logic lives in the core crate; this module only converts
//! at the boundary, returning plain records the Python wrapper turns into
//! polars frames.

use std::path::{Path, PathBuf};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use scipql_core::Error;

fn to_py_err(error: &Error) -> PyErr {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    PyValueError::new_err(message)
}

/// Run `rust-analyzer scip` on `project`, writing the index to `output`
/// (default `index.scip`). Returns the output path.
#[pyfunction]
#[pyo3(signature = (project, output = "index.scip"))]
fn index(project: &str, output: &str) -> PyResult<String> {
    scipql_core::index(Path::new(project), Path::new(output)).map_err(|error| to_py_err(&error))?;
    Ok(output.to_owned())
}

/// Lower a SCIP index into the four fact relations, each a list of row dicts:
/// `occurrence` {symbol, path, start, end, role}, `symbol_info`
/// {symbol, kind, `display_name`}, `document` {path}, `relationship`
/// {symbol, related, kind}.
#[pyfunction]
#[pyo3(signature = (index_path, root = None))]
fn facts<'py>(
    py: Python<'py>,
    index_path: &str,
    root: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let loaded = scipql_core::load_index(Path::new(index_path)).map_err(|error| to_py_err(&error))?;
    let root = root.map(PathBuf::from);
    let facts = scipql_core::facts_from_index(&loaded, root.as_deref())
        .map_err(|error| to_py_err(&error))?;

    let out = PyDict::new(py);

    let occurrences = PyList::empty(py);
    for row in &facts.occurrences {
        let cell = PyDict::new(py);
        cell.set_item("symbol", &row.symbol)?;
        cell.set_item("path", &row.path)?;
        cell.set_item("start", row.start)?;
        cell.set_item("end", row.end)?;
        cell.set_item("role", &row.role)?;
        occurrences.append(cell)?;
    }
    out.set_item("occurrence", occurrences)?;

    let symbols = PyList::empty(py);
    for row in &facts.symbols {
        let cell = PyDict::new(py);
        cell.set_item("symbol", &row.symbol)?;
        cell.set_item("kind", &row.kind)?;
        cell.set_item("display_name", &row.display_name)?;
        symbols.append(cell)?;
    }
    out.set_item("symbol_info", symbols)?;

    let documents = PyList::empty(py);
    for path in &facts.documents {
        let cell = PyDict::new(py);
        cell.set_item("path", path)?;
        documents.append(cell)?;
    }
    out.set_item("document", documents)?;

    let relationships = PyList::empty(py);
    for row in &facts.relationships {
        let cell = PyDict::new(py);
        cell.set_item("symbol", &row.symbol)?;
        cell.set_item("related", &row.related)?;
        cell.set_item("kind", &row.kind)?;
        relationships.append(cell)?;
    }
    out.set_item("relationship", relationships)?;

    Ok(out)
}

/// Run a Soufflé `program` over the index's facts. Returns
/// `{relation: {"columns": [name], "rows": [{column: value}]}}`, one entry per
/// `.output` relation. The fact relations are already in scope.
#[pyfunction]
#[pyo3(signature = (index_path, program, root = None))]
fn query<'py>(
    py: Python<'py>,
    index_path: &str,
    program: &str,
    root: Option<&str>,
) -> PyResult<Bound<'py, PyDict>> {
    let loaded = scipql_core::load_index(Path::new(index_path)).map_err(|error| to_py_err(&error))?;
    let root = root.map(PathBuf::from);
    let output = scipql_core::query(&loaded, root.as_deref(), program)
        .map_err(|error| to_py_err(&error))?;

    let out = PyDict::new(py);
    for relation in &output.relations {
        let entry = PyDict::new(py);
        entry.set_item("columns", &relation.columns)?;
        let rows = PyList::empty(py);
        for row in &relation.rows {
            let cells = PyDict::new(py);
            for (column, value) in relation.columns.iter().zip(row) {
                cells.set_item(column, value)?;
            }
            rows.append(cells)?;
        }
        entry.set_item("rows", rows)?;
        out.set_item(&relation.name, entry)?;
    }
    Ok(out)
}

/// Run a `fix` program (one that `.output`s `edit(path, start, end,
/// replacement)`) and return the unified diff. With `write=True` the files
/// under `root` are rewritten on disk.
#[pyfunction]
#[pyo3(signature = (index_path, program, root = None, write = false))]
fn fix(index_path: &str, program: &str, root: Option<&str>, write: bool) -> PyResult<String> {
    let loaded = scipql_core::load_index(Path::new(index_path)).map_err(|error| to_py_err(&error))?;
    let root = root.map(PathBuf::from);
    scipql_core::fix(&loaded, root.as_deref(), program, write).map_err(|error| to_py_err(&error))
}

/// Rename every occurrence whose SCIP moniker ends with `selector` to
/// `new_name`. Returns the unified diff; `write=True` applies it.
#[pyfunction]
#[pyo3(signature = (index_path, selector, new_name, root = None, write = false))]
fn rename(
    index_path: &str,
    selector: &str,
    new_name: &str,
    root: Option<&str>,
    write: bool,
) -> PyResult<String> {
    let loaded = scipql_core::load_index(Path::new(index_path)).map_err(|error| to_py_err(&error))?;
    let root = root.map(PathBuf::from);
    scipql_core::rename(&loaded, root.as_deref(), selector, new_name, write)
        .map_err(|error| to_py_err(&error))
}

#[pymodule]
fn _scipql(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(index, module)?)?;
    module.add_function(wrap_pyfunction!(facts, module)?)?;
    module.add_function(wrap_pyfunction!(query, module)?)?;
    module.add_function(wrap_pyfunction!(fix, module)?)?;
    module.add_function(wrap_pyfunction!(rename, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
