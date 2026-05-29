//! Python binding for the producer side.
//!
//! [`publish`] binds a unix socket in the discovery directory and streams this
//! process's terminals to whatever connects (the `tui-dashboard` aggregator).
//! The producer logic lives in the `tui` crate; this module only hands a
//! process-wide handle to Python. The returned [`Publisher`] keeps the socket
//! alive until stopped or dropped. Both `publish` and `stop` are awaitable so
//! the binding surface is uniformly async.

use std::path::PathBuf;
use std::time::Duration;

use parking_lot::Mutex;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::manager::global_manager;

/// Handle to a running producer. Dropping it or awaiting `stop` stops the
/// streaming loops and unlinks the socket.
#[pyclass(module = "tui._tui")]
pub struct Publisher {
    path: String,
    producer: String,
    inner: Mutex<Option<tui::Publisher>>,
}

#[pymethods]
impl Publisher {
    /// The socket path this producer is bound to.
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    /// This process's producer id: the scope its terminals appear under in the
    /// aggregated dashboard.
    #[getter]
    fn producer_id(&self) -> &str {
        &self.producer
    }

    /// Stop streaming, wait for the loops to wind down, and unlink the socket.
    /// Idempotent.
    fn stop<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // Take the value out under the lock before the await point so the guard
        // never crosses it; the wind-down then runs inside the future.
        let taken = self.inner.lock().take();
        future_into_py(py, async move {
            if let Some(mut publisher) = taken {
                publisher.stop().await;
            }
            Ok(())
        })
    }

    fn __repr__(&self) -> String {
        format!("Publisher(path={:?})", self.path)
    }
}

/// Publish the global manager's terminals over a unix socket.
///
/// With `path` unset the socket lands in the discovery directory under a
/// per-process name, where the `tui-dashboard` aggregator finds it. `poll_ms` is
/// the sampling interval in milliseconds.
#[pyfunction]
#[pyo3(signature = (path=None, poll_ms=100))]
pub fn publish(py: Python<'_>, path: Option<String>, poll_ms: u64) -> PyResult<Bound<'_, PyAny>> {
    let path = path.map_or_else(tui::socket_path, PathBuf::from);
    let manager = global_manager();
    future_into_py(py, async move {
        let publisher = tui::publish(&manager, path, Duration::from_millis(poll_ms)).await?;
        Ok(Publisher {
            path: publisher.path().display().to_string(),
            producer: publisher.producer_id().to_owned(),
            inner: Mutex::new(Some(publisher)),
        })
    })
}

/// The discovery directory where producers expose sockets and the aggregator
/// looks for them.
#[pyfunction]
pub fn socket_dir() -> String {
    tui::socket_dir().display().to_string()
}
