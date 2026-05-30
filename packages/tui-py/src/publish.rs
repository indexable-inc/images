//! Python binding for the producer side.
//!
//! [`publish`] binds a unix socket in the discovery directory and streams this
//! process's terminals to whatever connects (the `tui-dashboard` aggregator).
//! The producer logic lives in the `tui` crate; this module only hands a
//! process-wide handle to Python. The returned [`Publisher`] keeps the socket
//! alive until stopped or dropped. Both `publish` and `stop` are awaitable so
//! the binding surface is uniformly async.
//!
//! [`ensure_published`] is the synchronous twin that the high-level Python API
//! calls on first `Tui(...)`: it binds one process-global producer so terminals
//! show up in `tui-dashboard` with no explicit `tui.publish()`.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use pyo3::prelude::*;
use pyo3_async_runtimes::tokio::future_into_py;

use crate::manager::global_manager;

/// The process-global producer bound by [`ensure_published`].
///
/// It outlives every Python handle so the dashboard keeps seeing this process's
/// terminals for the life of the interpreter. The `OnceLock` makes the first
/// call the binder; the inner `Mutex<Option<_>>` holds the live producer (or
/// `None` once a bind was attempted and skipped or torn down).
static AUTOPUBLISHER: OnceLock<Mutex<Option<tui::Publisher>>> = OnceLock::new();

/// Set once any producer is bound for this process, by either [`publish`] or
/// [`ensure_published`]. Auto-publish checks it so an explicit
/// `tui.publish(...)` (chosen for a custom socket path or poll interval) is not
/// shadowed by a second process-global producer on the next `Tui(...)`, which
/// would make the aggregator list every terminal twice.
static PROCESS_PUBLISHED: AtomicBool = AtomicBool::new(false);

/// The env var that opts a process out of auto-publishing. The literal `"0"`
/// disables it; any other value (or unset) leaves auto-publish on.
const AUTOPUBLISH_OPT_OUT: &str = "IX_TUI_AUTOPUBLISH";

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
        // An explicit publish supersedes auto-publish: stop the process-global
        // producer if one was already bound, so the process exposes exactly one
        // producer rather than a duplicate under a second id. `take()` releases
        // the lock before the await.
        let previous = AUTOPUBLISHER.get().and_then(|slot| slot.lock().take());
        if let Some(mut previous) = previous {
            previous.stop().await;
        }
        let publisher = tui::publish(&manager, path, Duration::from_millis(poll_ms)).await?;
        // Mark the process published so a later `Tui(...)` does not auto-bind a
        // second producer on top of this explicit one.
        PROCESS_PUBLISHED.store(true, Ordering::Release);
        Ok(Publisher {
            path: publisher.path().display().to_string(),
            producer: publisher.producer_id().to_owned(),
            inner: Mutex::new(Some(publisher)),
        })
    })
}

/// Bind one process-global producer if none is running yet.
///
/// Synchronous and idempotent: the first call binds the producer on the global
/// manager's tokio runtime and stashes the handle in [`AUTOPUBLISHER`]; later
/// calls are a no-op. A no-op too when `IX_TUI_AUTOPUBLISH=0`. The high-level
/// `tui.Tui` calls this once on construction so spawned terminals appear in
/// `tui-dashboard` without an explicit `tui.publish()`.
///
/// The bind future is driven to completion on the pyo3-async-runtimes tokio
/// runtime under `py.detach` (releasing the GIL); `tui::publish` then runs its
/// own poll and accept loops on the manager's runtime, so the producer survives
/// past this call. A bind failure is swallowed: auto-publish is a convenience,
/// not a hard dependency, and an explicit `await tui.publish()` still surfaces
/// errors.
#[pyfunction]
#[pyo3(signature = (poll_ms=100))]
pub fn ensure_published(py: Python<'_>, poll_ms: u64) {
    if std::env::var(AUTOPUBLISH_OPT_OUT).as_deref() == Ok("0") {
        return;
    }
    // An explicit `tui.publish(...)` already exposed this process; do not bind a
    // second producer on top of it.
    if PROCESS_PUBLISHED.load(Ordering::Acquire) {
        return;
    }

    let slot = AUTOPUBLISHER.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock();
    if guard.is_some() {
        return;
    }

    let manager = global_manager();
    let runtime = pyo3_async_runtimes::tokio::get_runtime();
    let publisher = py.detach(|| {
        runtime.block_on(tui::publish(
            &manager,
            tui::socket_path(),
            Duration::from_millis(poll_ms),
        ))
    });
    if let Ok(publisher) = publisher {
        *guard = Some(publisher);
        PROCESS_PUBLISHED.store(true, Ordering::Release);
    }
}

/// The discovery directory where producers expose sockets and the aggregator
/// looks for them.
#[pyfunction]
pub fn socket_dir() -> String {
    tui::socket_dir().display().to_string()
}
