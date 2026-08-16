//! Boundary types exported code references at runtime.
//!
//! [`UniStream`] is the stream half of the unibind surface: an exported
//! `fn` returning `UniStream<T>` becomes an async iterator in the target
//! language, and items flow one poll per consumer request (pull-based
//! backpressure). Deliberately language-free: the per-language runtime
//! glue lives in `unibind-py-runtime` and `unibind-ex-runtime`, so this
//! crate can sit inside any binding artifact without dragging another
//! language's toolchain along.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use futures::StreamExt as _;

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

/// One consumer's pull handle over a [`UniStream`].
///
/// Holds the checked-out pull state, a gate serializing concurrent
/// pulls, and a level-triggered closed flag. The shared body of every
/// generated handle class, so the per-export types stay thin shells.
///
/// `close` is synchronous and race-free: the watch channel flips to
/// closed, which both wakes a pull blocked inside `next` (dropping the
/// checked-out stream) and keeps later pulls from checking the stream
/// back in.
pub struct PullStream<T> {
    stream: parking_lot::Mutex<Option<UniStream<T>>>,
    /// Gate serializing concurrent pulls.
    #[expect(
        clippy::disallowed_types,
        reason = "PullStream::next holds this guard across the tokio::select! \
                  that races the pull against close, so it must be an async \
                  mutex; parking_lot's guard is not Send across an await point"
    )]
    pull: tokio::sync::Mutex<()>,
    closed: tokio::sync::watch::Sender<bool>,
}

impl<T> PullStream<T> {
    /// Wrap `stream` for one consumer's serialized pulls.
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "initializes the `pull` field, which is a tokio mutex for the \
                  reason given on its declaration"
    )]
    pub fn new(stream: UniStream<T>) -> Self {
        Self {
            stream: parking_lot::Mutex::new(Some(stream)),
            pull: tokio::sync::Mutex::new(()),
            closed: tokio::sync::watch::Sender::new(false),
        }
    }

    fn slot(&self) -> parking_lot::MutexGuard<'_, Option<UniStream<T>>> {
        self.stream.lock()
    }

    /// The next element, or `None` once the stream ends or closes.
    pub async fn next(&self) -> Option<T> {
        #[expect(
            clippy::disallowed_methods,
            reason = "held across the tokio::select! below, which is why `pull` \
                      is a tokio mutex in the first place"
        )]
        let _pull = self.pull.lock().await;
        let mut stream = self.slot().take()?;
        let mut closed = self.closed.subscribe();
        let item = tokio::select! {
            biased;
            _ = closed.wait_for(|closed| *closed) => None,
            item = stream.next() => item,
        };
        if item.is_some() && !*self.closed.borrow() {
            self.slot().replace(stream);
        }
        item
    }

    /// Drop the stream early; a pull in flight resolves `None`, and the
    /// producer sees its stream dropped.
    pub fn close(&self) {
        // `send_replace`, not `send`: `send` reports Err and leaves the value
        // untouched when no pull is in flight to hold a receiver, so the flag
        // this type documents as "flips to closed" would stay clear.
        self.closed.send_replace(true);
        self.slot().take();
    }
}

// Opaque like `UniStream`, and for the same reason.
impl<T> fmt::Debug for PullStream<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("PullStream").finish_non_exhaustive()
    }
}
