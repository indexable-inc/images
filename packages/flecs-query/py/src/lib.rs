//! Python bindings for `flecs-query-core`.
//!
//! Three thin sync entry points: [`parse`] returns the AST as plain dicts
//! (via serde), [`canonicalize`] returns the normalized expression text, and
//! [`validate`] returns a non-raising verdict dict. All language behavior
//! lives in the core crate; this module only converts at the boundary.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

fn to_py_err(error: &flecs_query_core::ParseError, expr: &str) -> PyErr {
    PyValueError::new_err(error.render(expr))
}

/// Parse a Flecs Query Language expression into its AST as plain dicts.
///
/// Raises `ValueError` with a caret-rendered message on a syntax error.
#[pyfunction]
fn parse<'py>(py: Python<'py>, expr: &str) -> PyResult<Bound<'py, PyAny>> {
    let query = flecs_query_core::parse(expr).map_err(|error| to_py_err(&error, expr))?;
    pythonize::pythonize(py, &query).map_err(Into::into)
}

/// Parse an expression and return its canonical form.
///
/// Raises `ValueError` with a caret-rendered message on a syntax error.
#[pyfunction]
fn canonicalize(expr: &str) -> PyResult<String> {
    let query = flecs_query_core::parse(expr).map_err(|error| to_py_err(&error, expr))?;
    Ok(query.to_string())
}

/// Check whether a string is well-formed Flecs Query Language without
/// raising: returns `{"valid": bool, "error": str?, "rendered": str?}`.
#[pyfunction]
fn validate<'py>(py: Python<'py>, expr: &str) -> PyResult<Bound<'py, PyDict>> {
    let verdict = PyDict::new(py);
    match flecs_query_core::parse(expr) {
        Ok(_) => verdict.set_item("valid", true)?,
        Err(error) => {
            verdict.set_item("valid", false)?;
            verdict.set_item("error", error.to_string())?;
            verdict.set_item("rendered", error.render(expr))?;
        }
    }
    Ok(verdict)
}

#[pymodule]
fn _flecs_query(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(parse, module)?)?;
    module.add_function(wrap_pyfunction!(canonicalize, module)?)?;
    module.add_function(wrap_pyfunction!(validate, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
