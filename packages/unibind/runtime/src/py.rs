//! Python async helpers the generated glue calls.
//!
//! Everything here is a thin layer over `pyo3-async-runtimes`, so generated
//! code only ever names `unibind_runtime`.

use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt as _;

use pyo3::{Bound, PyAny, PyResult, Python};

use crate::UniStream;

/// Convert a Rust future into an awaitable Python object.
///
/// Single indirection point so generated glue only names `unibind-runtime`;
/// the asyncio-cancel-drops-the-future guarantee is inherited from
/// `pyo3-async-runtimes`.
///
/// The unwind is caught here because `pyo3-async-runtimes` reports a
/// panicking future as `RustPanic: rust future panicked: unknown error`,
/// discarding the payload; re-raising `pyo3::panic::PanicException` with
/// the panic text matches the sync boundary, where pyo3 itself raises
/// `PanicException` carrying the message.
///
/// # Errors
///
/// Fails when no asyncio event loop can be resolved for the calling
/// context.
pub fn future_into_py<'py, F, T>(py: Python<'py>, fut: F) -> PyResult<Bound<'py, PyAny>>
where
    F: Future<Output = PyResult<T>> + Send + 'static,
    T: for<'a> pyo3::IntoPyObject<'a> + Send + 'static,
{
    let caught = AssertUnwindSafe(fut).catch_unwind();
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        match caught.await {
            Ok(result) => result,
            Err(payload) => Err(pyo3::panic::PanicException::new_err(panic_text(
                payload.as_ref(),
            ))),
        }
    })
}

/// Best-effort panic payload text, mirroring std's default panic hook.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_owned())
}

/// A [`UniStream`] shared with Python's async iterator protocol.
///
/// `__anext__` is called on one shared object from whichever task drives
/// the iterator, so the stream sits behind a `tokio::sync::Mutex`: the
/// lock serializes polls and keeps the returned future `Send`.
pub struct SharedStream<T> {
    inner: Arc<tokio::sync::Mutex<UniStream<T>>>,
}

// Manual impl: cloning shares the underlying stream, so `T: Clone` is not
// required.
impl<T> Clone for SharedStream<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> SharedStream<T> {
    /// Wrap `stream` for shared consumption.
    #[must_use]
    pub fn new(stream: UniStream<T>) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(stream)),
        }
    }

    /// Pull the next item. The future owns its own `Arc`, so it outlives
    /// the `&self` borrow that produced it (pyo3 futures must be
    /// `'static`).
    pub fn next(&self) -> impl Future<Output = Option<T>> + Send + 'static + use<T>
    where
        T: Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        async move { inner.lock().await.next().await }
    }
}

impl<T> fmt::Debug for SharedStream<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("SharedStream").finish_non_exhaustive()
    }
}
