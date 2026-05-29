//! Python binding for the Loro-backed web dashboard.
//!
//! The whole dashboard (HTTP server, SSE stream, Loro document, poll loop)
//! lives in the `tui` crate behind its `dashboard` feature; this module only
//! hands a process-wide handle to Python. `serve` binds and starts the server
//! against the global manager; the returned [`Dashboard`] keeps it alive until
//! stopped or dropped. Both `serve` and `stop` are awaitable so the binding
//! surface is uniformly async.

use std::net::SocketAddr;
use std::time::Duration;

use parking_lot::Mutex;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::manager::global_manager;

/// Handle to a running dashboard server. Dropping it or awaiting `stop` shuts
/// the HTTP server and poll loop down.
#[pyclass(module = "tui._tui")]
pub struct Dashboard {
    addr: String,
    url: String,
    inner: Mutex<Option<tui::Dashboard>>,
}

#[pymethods]
impl Dashboard {
    /// The bound address, including the resolved port when `0` was requested.
    #[getter]
    fn addr(&self) -> &str {
        &self.addr
    }

    /// The URL to open in a browser.
    #[getter]
    fn url(&self) -> &str {
        &self.url
    }

    /// Stop the server and wait for its tasks to wind down. Idempotent.
    fn stop<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // Take the value out under the lock before the await point so the guard
        // never crosses it; the blocking wind-down then runs inside the future.
        let taken = self.inner.lock().take();
        future_into_py(py, async move {
            if let Some(mut dashboard) = taken {
                dashboard.stop();
            }
            Ok(())
        })
    }

    fn __repr__(&self) -> String {
        format!("Dashboard(url={:?})", self.url)
    }
}

/// Start the dashboard for the global manager's live terminals.
///
/// `host` must be an IP literal (a hostname is not resolved). Pass port `0` to
/// bind an ephemeral port and read it back from `Dashboard.url`. `poll_ms` is
/// the viewport sampling interval in milliseconds.
#[pyfunction]
#[pyo3(signature = (host="127.0.0.1", port=8080, poll_ms=100))]
pub fn serve<'py>(
    py: Python<'py>,
    host: &str,
    port: u16,
    poll_ms: u64,
) -> PyResult<Bound<'py, PyAny>> {
    let addr: SocketAddr = format!("{host}:{port}").parse().map_err(|source| {
        PyValueError::new_err(format!("invalid address {host}:{port}: {source}"))
    })?;

    let manager = global_manager();
    future_into_py(py, async move {
        let dashboard = tui::serve(&manager, addr, Duration::from_millis(poll_ms))?;
        Ok(Dashboard {
            addr: dashboard.addr().to_string(),
            url: dashboard.url(),
            inner: Mutex::new(Some(dashboard)),
        })
    })
}
