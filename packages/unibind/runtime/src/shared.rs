//! Pieces the Python and JVM async backends share.
//!
//! Lives behind `any(py, jvm)` rather than in the crate body so a
//! backend-less build never pulls `tokio` in; both backends re-export or
//! call into these, keeping one definition of the boundary stream wrapper
//! and the panic-text convention.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use crate::UniStream;

/// Best-effort panic payload text, mirroring std's default panic hook.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_owned())
}

/// A [`UniStream`] shared between boundary pulls.
///
/// Each pull is issued against one shared object from whichever task
/// drives the iteration, so the stream sits behind a `tokio::sync::Mutex`:
/// the lock serializes polls and keeps the returned future `Send`.
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
    /// the `&self` borrow that produced it (both backends need `'static`
    /// pull futures: pyo3 coroutines and spawned JVM pulls alike).
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
