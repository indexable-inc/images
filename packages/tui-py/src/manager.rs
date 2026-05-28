use std::sync::{Arc, OnceLock};

use ndarray::Array2;
use numpy::PyArray2;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::types::{FullOutput, StyledCell};

/// Single process-wide manager. Owns the tokio runtime that drives every
/// spawned PTY actor, and is held alive for the lifetime of the process.
static MANAGER: OnceLock<Arc<tui::TuiManager>> = OnceLock::new();

fn global_manager() -> Arc<tui::TuiManager> {
    MANAGER
        .get_or_init(|| Arc::new(tui::TuiManager::new()))
        .clone()
}

/// Low-level binding around `tui::TuiInstance`. Spawning constructs the
/// underlying PTY child and registers it with the global manager; the
/// `Arc<tui::TuiManager>` field keeps that manager (and its runtime) alive
/// for as long as Python holds a handle.
#[pyclass(frozen, module = "tui._tui")]
pub struct TuiInstance {
    inner: tui::TuiInstance,
    #[allow(dead_code, reason = "lifetime anchor for the manager runtime")]
    manager: Arc<tui::TuiManager>,
}

#[pymethods]
impl TuiInstance {
    /// Spawn `command` with the given positional args, attached to a fresh PTY.
    #[new]
    #[pyo3(signature = (command, args=None, scrollback_lines=10_000))]
    fn new(
        py: Python<'_>,
        command: String,
        args: Option<Vec<String>>,
        scrollback_lines: usize,
    ) -> PyResult<Self> {
        let manager = global_manager();
        let args = args.unwrap_or_default();
        let m = Arc::clone(&manager);
        let inner = py.detach(move || m.spawn(command, args, scrollback_lines))?;
        Ok(Self { inner, manager })
    }

    /// All currently tracked instances.
    #[staticmethod]
    fn list_all() -> Vec<Self> {
        let manager = global_manager();
        manager
            .list()
            .into_iter()
            .map(|inner| Self {
                inner,
                manager: Arc::clone(&manager),
            })
            .collect()
    }

    // -- identity / shape -------------------------------------------------

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

    // -- sync I/O ---------------------------------------------------------

    fn write(&self, py: Python<'_>, data: &str) -> PyResult<()> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let owned = data.to_owned();
        py.detach(move || manager.write(&inner, &owned))?;
        Ok(())
    }

    fn read_viewport(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let lines = py.detach(move || manager.read_viewport(&inner))?;
        Ok(lines)
    }

    fn read_scrollback(&self, py: Python<'_>) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let lines = py.detach(move || manager.read_scrollback(&inner))?;
        Ok(lines)
    }

    fn read_full(&self, py: Python<'_>) -> PyResult<FullOutput> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let full = py.detach(move || manager.read_full(&inner))?;
        Ok(full.into())
    }

    fn read_blocking(&self, py: Python<'_>, timeout_ms: u64) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let lines = py.detach(move || manager.read_blocking(&inner, timeout_ms))?;
        Ok(lines)
    }

    fn read_chars_array<'py>(
        &self,
        py: Python<'py>,
    ) -> PyResult<Bound<'py, PyArray2<u32>>> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let rows = py.detach(move || manager.read_chars(&inner))?;
        chars_to_array(py, rows)
    }

    fn read_styled_cells(&self, py: Python<'_>) -> PyResult<Vec<Vec<StyledCell>>> {
        let inner = self.inner.clone();
        let manager = Arc::clone(&self.manager);
        let cells = py.detach(move || manager.read_styled_cells(&inner))?;
        Ok(styled_to_nested(&cells))
    }

    // -- async I/O (returns asyncio-awaitable coroutines) -----------------

    fn write_async<'py>(&self, py: Python<'py>, data: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            tui::TuiManager::write_async(&inner, &data).await?;
            Ok(())
        })
    }

    fn read_viewport_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = tui::TuiManager::read_viewport_async(&inner).await?;
            Ok(lines)
        })
    }

    fn read_scrollback_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = tui::TuiManager::read_scrollback_async(&inner).await?;
            Ok(lines)
        })
    }

    fn read_full_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let full = tui::TuiManager::read_full_async(&inner).await?;
            let out: FullOutput = full.into();
            Ok(out)
        })
    }

    fn read_blocking_async<'py>(
        &self,
        py: Python<'py>,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = tui::TuiManager::read_blocking_async(&inner, timeout_ms).await?;
            Ok(lines)
        })
    }

    fn read_chars_array_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let rows = tui::TuiManager::read_chars_async(&inner).await?;
            Python::attach(|py| {
                let arr = chars_to_array(py, rows)?;
                Ok(arr.unbind())
            })
        })
    }

    fn read_styled_cells_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let cells = tui::TuiManager::read_styled_cells_async(&inner).await?;
            Ok(styled_to_nested(&cells))
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "_TuiInstance(id={}, command={:?}, args={:?}, rows={}, cols={})",
            self.inner.id, self.inner.command, self.inner.args, self.inner.rows, self.inner.cols,
        )
    }
}

fn chars_to_array(
    py: Python<'_>,
    rows: Vec<Vec<char>>,
) -> PyResult<Bound<'_, PyArray2<u32>>> {
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

fn styled_to_nested(cells: &Array2<tui::StyledCell>) -> Vec<Vec<StyledCell>> {
    let (row_count, _) = cells.dim();
    let mut out = Vec::with_capacity(row_count);
    for row in cells.rows() {
        out.push(row.iter().cloned().map(StyledCell::from).collect());
    }
    out
}
