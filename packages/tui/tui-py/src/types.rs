use pyo3::IntoPyObject;
use pyo3::prelude::*;

use tui::Color;

/// One styled terminal cell exposed to Python.
///
/// `fg` and `bg` use the pythonic color encoding: `None` is the terminal
/// default, an `int` in `0..=255` is a palette index, and an `(r, g, b)` tuple
/// is 24-bit truecolor.
#[pyclass(frozen, module = "tui._tui")]
pub struct StyledCell {
    inner: tui::StyledCell,
}

impl From<tui::StyledCell> for StyledCell {
    fn from(inner: tui::StyledCell) -> Self {
        Self { inner }
    }
}

/// Convert a core color into its Python value: `None`, an `int`, or a tuple.
fn color_to_py(py: Python<'_>, color: Color) -> PyResult<Py<PyAny>> {
    match color {
        Color::Default => Ok(py.None()),
        Color::Indexed(index) => Ok(index.into_pyobject(py)?.into_any().unbind()),
        Color::Rgb(r, g, b) => Ok((r, g, b).into_pyobject(py)?.into_any().unbind()),
    }
}

/// Render a color the way the Python value prints, for use in `__repr__`.
fn color_repr(color: Color) -> String {
    match color {
        Color::Default => "None".to_string(),
        Color::Indexed(index) => index.to_string(),
        Color::Rgb(r, g, b) => format!("({r}, {g}, {b})"),
    }
}

#[pymethods]
impl StyledCell {
    #[getter]
    fn char(&self) -> String {
        self.inner.character.to_string()
    }

    #[getter]
    fn fg(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        color_to_py(py, self.inner.fg)
    }

    #[getter]
    fn bg(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        color_to_py(py, self.inner.bg)
    }

    #[getter]
    fn bold(&self) -> bool {
        self.inner.bold
    }

    #[getter]
    fn italic(&self) -> bool {
        self.inner.italic
    }

    #[getter]
    fn underline(&self) -> bool {
        self.inner.underline
    }

    #[getter]
    fn inverse(&self) -> bool {
        self.inner.inverse
    }

    fn __repr__(&self) -> String {
        format!(
            "StyledCell(char={:?}, fg={}, bg={}, bold={}, italic={}, underline={}, inverse={})",
            self.inner.character,
            color_repr(self.inner.fg),
            color_repr(self.inner.bg),
            self.inner.bold,
            self.inner.italic,
            self.inner.underline,
            self.inner.inverse,
        )
    }
}
