//! Boundary types exported code references at runtime.
//!
//! [`UniStream`] is the stream half of the unibind surface: an exported
//! `fn` returning `UniStream<T>` becomes an async iterator in the target
//! language, and items flow one poll per consumer request (pull-based
//! backpressure). The `py` feature adds the Python async helpers the
//! generated glue calls into.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use futures::StreamExt as _;

#[cfg(feature = "py")]
pub mod py;

/// A boxed stream crossing the binding boundary.
///
/// Exported functions return it by value; the backend wraps it in the
/// target language's async iterator, so each `__anext__` (or equivalent)
/// pulls exactly one item and the producer sees backpressure for free.
pub struct UniStream<T> {
    inner: Pin<Box<dyn Stream<Item = T> + Send + 'static>>,
}

impl<T> UniStream<T> {
    /// Box `stream` for the boundary.
    #[must_use]
    pub fn new(stream: impl Stream<Item = T> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Pull the next item; `None` once the stream ends.
    pub async fn next(&mut self) -> Option<T> {
        self.inner.next().await
    }
}

impl<T> Stream for UniStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.inner.as_mut().poll_next(cx)
    }
}

// Opaque by hand: the boxed stream has no useful state to show, and a
// derive would demand `T: Debug` from every exported item type.
impl<T> fmt::Debug for UniStream<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("UniStream").finish_non_exhaustive()
    }
}
