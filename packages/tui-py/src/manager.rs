use std::sync::Arc;

use numpy::PyArray2;
use pyo3::prelude::*;

use crate::types::{FullOutput, StyledCell};

/// A handle to a single spawned TUI process.
#[pyclass(frozen, from_py_object, module = "superglide_tui._superglide_tui")]
#[derive(Clone)]
pub struct TuiInstance {
    pub(crate) inner: tui::TuiInstance,
}

#[pymethods]
impl TuiInstance {
    #[getter]
    fn id(&self) -> String {
        self.inner.id.to_string()
    }

    #[getter]
    fn command(&self) -> &str {
        &self.inner.command
    }

    #[getter]
    fn args(&self) -> Vec<String> {
        self.inner.args.clone()
    }

    #[getter]
    fn cols(&self) -> u16 {
        self.inner.cols
    }

    #[getter]
    fn rows(&self) -> u16 {
        self.inner.rows
    }

    #[getter]
    fn scrollback_limit(&self) -> usize {
        self.inner.scrollback_limit
    }

    fn __repr__(&self) -> String {
        format!(
            "TuiInstance(id={}, command={:?}, args={:?}, rows={}, cols={})",
            self.inner.id, self.inner.command, self.inner.args, self.inner.rows, self.inner.cols,
        )
    }
}

/// Manages many concurrent PTY-backed TUI processes.
#[pyclass(module = "superglide_tui._superglide_tui")]
pub struct TuiManager {
    inner: Arc<tui::TuiManager>,
}

#[pymethods]
impl TuiManager {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(tui::TuiManager::new()),
        }
    }

    /// Spawn a child process attached to a fresh PTY.
    #[pyo3(signature = (command, args=None, scrollback_lines=10_000))]
    fn spawn(
        &self,
        py: Python<'_>,
        command: String,
        args: Option<Vec<String>>,
        scrollback_lines: usize,
    ) -> PyResult<TuiInstance> {
        let manager = Arc::clone(&self.inner);
        let args = args.unwrap_or_default();
        let instance = py.detach(move || manager.spawn(command, args, scrollback_lines))?;
        Ok(TuiInstance { inner: instance })
    }

    /// All currently tracked instances.
    fn list(&self) -> Vec<TuiInstance> {
        self.inner
            .list()
            .into_iter()
            .map(|inner| TuiInstance { inner })
            .collect()
    }

    /// Send raw input bytes (UTF-8) to the PTY.
    fn write(&self, py: Python<'_>, instance: &TuiInstance, data: &str) -> PyResult<()> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        py.detach(move || manager.write(&inner, data))?;
        Ok(())
    }

    /// Read the current viewport (24x80 by default) as a list of lines.
    fn read(&self, py: Python<'_>, instance: &TuiInstance) -> PyResult<Vec<String>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let lines = py.detach(move || manager.read(&inner))?;
        Ok(lines)
    }

    fn read_viewport(&self, py: Python<'_>, instance: &TuiInstance) -> PyResult<Vec<String>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let lines = py.detach(move || manager.read_viewport(&inner))?;
        Ok(lines)
    }

    fn read_scrollback(&self, py: Python<'_>, instance: &TuiInstance) -> PyResult<Vec<String>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let lines = py.detach(move || manager.read_scrollback(&inner))?;
        Ok(lines)
    }

    /// Block until some output is available or until `timeout_ms` elapses.
    fn read_blocking(
        &self,
        py: Python<'_>,
        instance: &TuiInstance,
        timeout_ms: u64,
    ) -> PyResult<Vec<String>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let lines = py.detach(move || manager.read_blocking(&inner, timeout_ms))?;
        Ok(lines)
    }

    /// Combined snapshot of scrollback + viewport.
    fn read_full(&self, py: Python<'_>, instance: &TuiInstance) -> PyResult<FullOutput> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let full = py.detach(move || manager.read_full(&inner))?;
        Ok(full.into())
    }

    /// Per-cell characters of the current viewport as a nested list.
    fn read_chars(
        &self,
        py: Python<'_>,
        instance: &TuiInstance,
    ) -> PyResult<Vec<Vec<String>>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let rows = py.detach(move || manager.read_chars(&inner))?;
        Ok(rows
            .into_iter()
            .map(|row| row.into_iter().map(|ch| ch.to_string()).collect())
            .collect())
    }

    /// Per-cell characters of the current viewport as a uint32 numpy array of
    /// Unicode codepoints, shape `(rows, cols)`.
    fn read_chars_array<'py>(
        &self,
        py: Python<'py>,
        instance: &TuiInstance,
    ) -> PyResult<Bound<'py, PyArray2<u32>>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let rows = py.detach(move || manager.read_chars(&inner))?;

        let codepoint_rows: Vec<Vec<u32>> = rows
            .into_iter()
            .map(|row| row.into_iter().map(|ch| ch as u32).collect())
            .collect();

        PyArray2::from_vec2(py, &codepoint_rows).map_err(|source| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "failed to build char array: {source}"
            ))
        })
    }

    /// Per-cell styled view of the current viewport as a nested list of
    /// `StyledCell` objects.
    fn read_styled_cells(
        &self,
        py: Python<'_>,
        instance: &TuiInstance,
    ) -> PyResult<Vec<Vec<StyledCell>>> {
        let manager = Arc::clone(&self.inner);
        let inner = instance.inner.clone();
        let cells = py.detach(move || manager.read_styled_cells(&inner))?;

        let (row_count, _) = cells.dim();
        let mut out = Vec::with_capacity(row_count);
        for row in cells.rows() {
            out.push(row.iter().cloned().map(StyledCell::from).collect());
        }
        Ok(out)
    }
}
