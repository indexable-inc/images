use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ndarray::Array2;
use numpy::PyArray2;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::types::StyledCell;

/// Single process-wide manager. Owns the tokio runtime that drives every
/// spawned PTY actor, and is held alive for the lifetime of the process.
static MANAGER: OnceLock<Arc<tui::TuiManager>> = OnceLock::new();

pub fn global_manager() -> Arc<tui::TuiManager> {
    MANAGER
        .get_or_init(|| Arc::new(tui::TuiManager::new()))
        .clone()
}

/// Low-level binding around `tui::TuiInstance`. Spawning constructs the
/// underlying PTY child and registers it with the global manager. The wrapped
/// `tui::TuiInstance` holds its own clone of the manager's runtime, so the
/// runtime stays alive for as long as Python holds this handle.
///
/// Every I/O method returns a native asyncio-awaitable coroutine bridged
/// through pyo3-async-runtimes; the high-level `tui.Tui` wrapper is async-only,
/// so there are no GIL-releasing sync twins here. Only the pure accessors
/// (`is_alive`, `exit_code`, the shape getters) stay synchronous.
#[pyclass(frozen, module = "tui._tui")]
pub struct TuiInstance {
    inner: tui::TuiInstance,
}

#[pymethods]
impl TuiInstance {
    /// Spawn `command` on a fresh PTY. Unset size or scrollback fall back to
    /// the core defaults (80x24, 10,000 lines).
    #[new]
    #[pyo3(signature = (command, args=None, rows=None, cols=None, scrollback_lines=None))]
    fn new(
        py: Python<'_>,
        command: String,
        args: Option<Vec<String>>,
        rows: Option<u16>,
        cols: Option<u16>,
        scrollback_lines: Option<usize>,
    ) -> PyResult<Self> {
        let manager = global_manager();
        let args = args.unwrap_or_default();

        let mut config = tui::SpawnConfig::default();
        if let Some(rows) = rows {
            config.rows = rows;
        }
        if let Some(cols) = cols {
            config.cols = cols;
        }
        if let Some(scrollback_lines) = scrollback_lines {
            config.scrollback_lines = scrollback_lines;
        }

        let inner = py.detach(move || manager.spawn(command, args, config))?;
        Ok(Self { inner })
    }

    /// All currently tracked instances.
    #[staticmethod]
    fn list_all() -> Vec<Self> {
        global_manager()
            .list()
            .into_iter()
            .map(|inner| Self { inner })
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
        self.inner.cols()
    }

    #[getter]
    fn rows(&self) -> u16 {
        self.inner.rows()
    }

    #[getter]
    fn scrollback_limit(&self) -> usize {
        self.inner.scrollback_limit
    }

    /// Whether the child process is still running. Reads cached state, so it
    /// stays synchronous.
    fn is_alive(&self) -> bool {
        self.inner.is_alive()
    }

    /// The exit code, or `None` while running or if terminated by a signal.
    /// Reads cached state, so it stays synchronous.
    fn exit_code(&self) -> Option<i32> {
        match self.inner.exit_state() {
            tui::ExitState::Exited(code) => code,
            tui::ExitState::Running => None,
        }
    }

    // -- async I/O (native asyncio-awaitable coroutines) ------------------

    fn write_async<'py>(&self, py: Python<'py>, data: String) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner.write_async(&data).await?;
            Ok(())
        })
    }

    fn read_viewport_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = inner.read_viewport_async().await?;
            Ok(lines)
        })
    }

    fn read_scrollback_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = inner.read_scrollback_async().await?;
            Ok(lines)
        })
    }

    fn read_full_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let full = inner.read_full_async().await?;
            Ok((full.scrollback, full.viewport))
        })
    }

    fn read_blocking_async<'py>(
        &self,
        py: Python<'py>,
        timeout_ms: u64,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let lines = inner
                .read_blocking_async(Duration::from_millis(timeout_ms))
                .await?;
            Ok(lines)
        })
    }

    fn read_chars_array_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let rows = inner.read_chars_async().await?;
            Python::attach(|py| {
                let arr = chars_to_array(py, rows)?;
                Ok(arr.unbind())
            })
        })
    }

    fn read_styled_cells_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let cells = inner.read_styled_cells_async().await?;
            Ok(styled_to_nested(&cells))
        })
    }

    /// Resize the terminal to `rows` x `cols` (delivers SIGWINCH to the child).
    fn resize_async<'py>(
        &self,
        py: Python<'py>,
        rows: u16,
        cols: u16,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner.resize_async(rows, cols).await?;
            Ok(())
        })
    }

    /// Force-terminate the child with `SIGKILL`. A no-op if already exited.
    fn kill_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            inner.kill_async().await?;
            Ok(())
        })
    }

    /// Await the child's exit, returning its exit code (`None` if signaled).
    fn wait_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        future_into_py(py, async move {
            let code = match inner.wait_async().await {
                tui::ExitState::Exited(code) => code,
                tui::ExitState::Running => None,
            };
            Ok(code)
        })
    }

    /// Force-kill the child and stop tracking it, dropping it from `list_all`
    /// and the dashboard. Tolerates an already-exited child.
    fn close_async<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let id = self.inner.id;
        future_into_py(py, async move {
            let _ = inner.kill_async().await;
            let _ = global_manager().remove(&id);
            Ok(())
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "_TuiInstance(id={}, command={:?}, args={:?}, rows={}, cols={})",
            self.inner.id,
            self.inner.command,
            self.inner.args,
            self.inner.rows(),
            self.inner.cols(),
        )
    }
}

fn chars_to_array(py: Python<'_>, rows: Vec<Vec<char>>) -> PyResult<Bound<'_, PyArray2<u32>>> {
    let codepoint_rows: Vec<Vec<u32>> = rows
        .into_iter()
        .map(|row| row.into_iter().map(u32::from).collect())
        .collect();

    PyArray2::from_vec2(py, &codepoint_rows).map_err(|source| {
        pyo3::exceptions::PyRuntimeError::new_err(format!("failed to build char array: {source}"))
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
