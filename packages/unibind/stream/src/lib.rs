//! The ABI-stable stream protocol shared by unibind engines and generated
//! clients.
//!
//! stabby has no stream type, so this crate declares one the same way
//! stabby-abi declares its ABI-stable `Future`: a `#[stabby::stabby]` trait
//! with `extern "C"` methods, an adapter over real
//! [`futures_core::Stream`]s, and a `dynptr` alias. It must be one crate
//! that *both* sides of the boundary depend on: stabby's structural type
//! report stamps a trait vtable with the module path of its declaration
//! site, so an engine and a client that each declared their own copy would
//! fail `get_stabbied`'s report check even though the shapes match. (Record
//! mirrors dodge the same trap with `#[stabby::stabby(module = ...)]`, but
//! the trait macro offers no such override.)
//!
//! The protocol is two calls rather than a `poll_next` returning
//! `Option<Option<Item>>`: stabby's niche-determinant machinery cannot
//! prove a *generic* nested option stable at the trait boundary (the same
//! reason stabby's own `Future` returns a single `Option`). `poll_next`'s
//! `None` means "no item right now"; [`RawStream::is_done`] disambiguates
//! end-of-stream from pending.

use core::pin::Pin;
use core::task::{Context, Poll};

use stabby::abi::IDeterminantProvider;
use stabby::future::StableWaker;
use stabby::option::Option;

/// A boxed stream crossing the binding boundary.
///
/// Exported functions return it by value; each backend wraps it in the
/// target language's async iterator (Python's `__anext__`, the generated
/// Rust client's `futures_core::Stream` wrapper), so every pull crosses the
/// boundary once and the producer sees backpressure for free. It lives here
/// rather than in `unibind-runtime` so a Rust-ABI engine never links the
/// runtime's optional pyo3 half: cargo unifies features workspace-wide, and
/// a py-enabled runtime rlib would drag Python's C symbols into every
/// engine cdylib link.
pub struct UniStream<T> {
    inner: Pin<std::boxed::Box<dyn futures_core::Stream<Item = T> + Send + 'static>>,
}

impl<T> UniStream<T> {
    /// Box `stream` for the boundary.
    #[must_use]
    pub fn new(stream: impl futures_core::Stream<Item = T> + Send + 'static) -> Self {
        Self {
            inner: std::boxed::Box::pin(stream),
        }
    }

    /// Pull the next item; `None` once the stream ends.
    pub async fn next(&mut self) -> core::option::Option<T> {
        core::future::poll_fn(|context| self.inner.as_mut().poll_next(context)).await
    }
}

impl<T> futures_core::Stream for UniStream<T> {
    type Item = T;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<core::option::Option<T>> {
        self.inner.as_mut().poll_next(context)
    }
}

// Opaque by hand: the boxed stream has no useful state to show, and a
// derive would demand `T: Debug` from every exported item type.
impl<T> core::fmt::Debug for UniStream<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("UniStream").finish_non_exhaustive()
    }
}

/// [`futures_core::Stream`], but ABI-stable. `None` from `poll_next` plus
/// `is_done() == false` is "pending"; with `is_done() == true` the stream
/// has ended.
#[stabby::stabby]
pub trait RawStream {
    /// The stream's item type.
    type Item: IDeterminantProvider<()>;
    /// Poll for the next item; `None` when no item is ready right now.
    extern "C" fn poll_next<'a>(&'a mut self, waker: StableWaker<'a>) -> Option<Self::Item>;
    /// Whether the stream has ended; meaningful after a `None` poll.
    extern "C" fn is_done(&self) -> bool;
}

/// Adapts a real [`futures_core::Stream`] to the raw protocol.
///
/// Tracks termination for [`RawStream::is_done`]; engines box one of these
/// into a [`DynStream`], which the generated glue does for every exported
/// stream return.
pub struct StreamAdapter<S> {
    stream: S,
    done: bool,
}

impl<S> StreamAdapter<S> {
    /// Wrap `stream`; nothing is polled until the boxed adapter is.
    pub const fn new(stream: S) -> Self {
        Self {
            stream,
            done: false,
        }
    }
}

impl<S: futures_core::Stream> RawStream for StreamAdapter<S>
where
    S::Item: IDeterminantProvider<()>,
{
    type Item = S::Item;

    #[allow(improper_ctypes_definitions)]
    extern "C" fn poll_next<'a>(&'a mut self, waker: StableWaker<'a>) -> Option<Self::Item> {
        if self.done {
            return Option::None();
        }
        // SAFETY: mirrors stabby's blanket `Future` impl. The adapter lives
        // behind a stable `Box` inside a `Dyn` pointer, which never moves
        // its pointee, so pinning the field here is sound.
        let stream = unsafe { core::pin::Pin::new_unchecked(&mut self.stream) };
        let polled = waker.with_waker(|waker| {
            futures_core::Stream::poll_next(stream, &mut core::task::Context::from_waker(waker))
        });
        match polled {
            core::task::Poll::Ready(core::option::Option::Some(item)) => Option::Some(item),
            core::task::Poll::Ready(core::option::Option::None) => {
                self.done = true;
                Option::None()
            }
            core::task::Poll::Pending => Option::None(),
        }
    }

    extern "C" fn is_done(&self) -> bool {
        self.done
    }
}

/// The shape unibind engines return streams as.
///
/// A type alias for `dynptr!(Box<dyn RawStream<Item = Item> + Send + 'a>)`.
/// `Send` only: the engine side wraps [`UniStream`], whose boxed inner
/// stream is deliberately not `Sync` (a stream is polled from one place at
/// a time).
pub type DynStream<'a, Item> =
    stabby::dynptr!(stabby::boxed::Box<dyn RawStream<Item = Item> + Send + 'a>);

/// Poll one item out of a [`DynStream`], bridging a std waker across the
/// ABI. Generated client wrappers build their [`futures_core::Stream`]
/// impls on this.
pub fn poll_next<Item: IDeterminantProvider<()>>(
    stream: &mut DynStream<'static, Item>,
    context: &mut core::task::Context<'_>,
) -> core::task::Poll<core::option::Option<Item>> {
    // Method syntax resolves through the extension traits the
    // `#[stabby::stabby]` trait macro generates alongside the trait.
    let polled: core::option::Option<Item> = stream.poll_next(context.waker().into()).into();
    match polled {
        core::option::Option::Some(item) => {
            core::task::Poll::Ready(core::option::Option::Some(item))
        }
        core::option::Option::None if stream.is_done() => {
            core::task::Poll::Ready(core::option::Option::None)
        }
        core::option::Option::None => core::task::Poll::Pending,
    }
}

#[cfg(test)]
mod tests {
    use core::task::{Context, Poll, Waker};

    use super::{DynStream, StreamAdapter};

    /// Yields `0..limit`, returning `Pending` (with a wake) between items.
    struct CountTo {
        next: u64,
        limit: u64,
        ready: bool,
    }

    impl futures_core::Stream for CountTo {
        type Item = u64;

        fn poll_next(
            mut self: core::pin::Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<u64>> {
            if self.next >= self.limit {
                return Poll::Ready(None);
            }
            if self.ready {
                self.ready = false;
                let item = self.next;
                self.next += 1;
                return Poll::Ready(Some(item));
            }
            self.ready = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    #[test]
    fn dyn_stream_round_trips_items_and_end() {
        let mut stream: DynStream<'static, u64> =
            stabby::boxed::Box::new(StreamAdapter::new(CountTo {
                next: 0,
                limit: 3,
                ready: false,
            }))
            .into();
        let mut context = Context::from_waker(Waker::noop());
        let mut seen = Vec::new();
        let mut pendings = 0;
        loop {
            match super::poll_next(&mut stream, &mut context) {
                Poll::Ready(Some(item)) => seen.push(item),
                Poll::Ready(None) => break,
                Poll::Pending => pendings += 1,
            }
        }
        assert_eq!(seen, [0, 1, 2]);
        assert_eq!(pendings, 3, "one yield per item");
    }
}
