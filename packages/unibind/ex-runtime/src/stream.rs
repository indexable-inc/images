//! Demand-driven streams across the BEAM boundary.

use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use futures_core::Stream as _;

use rustler::env::OwnedEnv;
use rustler::{Encoder, Env, LocalPid, Monitor, NifResult, Resource, ResourceArc, Term};
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;

use crate::atoms;
use crate::reply::{abort_stored, store_and_monitor};
use crate::runtime::runtime;

/// A boxed stream of items headed for the BEAM: the return type of a
/// unibind stream function.
pub struct Stream<T>(pub Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'static>>);

impl<T> Stream<T> {
    /// Box `inner` as a unibind stream.
    #[must_use]
    pub fn new(inner: impl futures_core::Stream<Item = T> + Send + 'static) -> Self {
        Self(Box::pin(inner))
    }

    /// A stream that yields the iterator's items, each ready immediately.
    ///
    /// Not `FromIterator`: the adapter needs `I::IntoIter: Send`, which the
    /// trait cannot express.
    #[must_use]
    pub fn from_iterator<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: Send + 'static,
    {
        Self::new(IterStream(iter.into_iter()))
    }

    /// The next item, `None` once the stream ends.
    pub async fn next(&mut self) -> Option<T> {
        std::future::poll_fn(|cx| self.0.as_mut().poll_next(cx)).await
    }
}

/// Adapts an iterator into an always-ready stream.
struct IterStream<I>(I);

/// The iterator is never pinned structurally, so the adapter moves freely.
impl<I> Unpin for IterStream<I> {}

impl<I: Iterator> futures_core::Stream for IterStream<I> {
    type Item = I::Item;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.next())
    }
}

/// The consumer-facing handle of a running stream: the credit counter the
/// generated `unibind_demand` NIF feeds, and the producer's abort handle.
pub struct StreamHandle {
    credits: Semaphore,
    abort: Mutex<Option<AbortHandle>>,
}

#[rustler::resource_impl]
impl Resource for StreamHandle {
    fn down<'a>(&'a self, _env: Env<'a>, _pid: LocalPid, _monitor: Monitor) {
        abort_stored(&self.abort);
    }
}

/// Grant the producer demand for `n` more items, saturating at the
/// semaphore's capacity.
pub fn grant(handle: &StreamHandle, n: u64) {
    let room = Semaphore::MAX_PERMITS.saturating_sub(handle.credits.available_permits());
    // Demand beyond the semaphore's capacity is indistinguishable from
    // unbounded demand, so grants clamp to the room left.
    let granted = usize::try_from(n).map_or(room, |n| n.min(room));
    handle.credits.add_permits(granted);
}

/// Drive `stream` on the shared runtime, sending the calling process one
/// `{:unibind_stream, reference, {:item, value}}` per granted credit and a
/// final `{:unibind_stream, reference, :done}`.
///
/// Failures of a throwing stream function happen before the stream exists,
/// so the producer itself never sends an error message.
///
/// The producer stops when the caller exits (monitor) or disappears
/// mid-send; either way the stream is dropped, observable to the user only
/// as their `Drop` impls running.
///
/// # Errors
///
/// Never fails today; the `NifResult` keeps the signature open for
/// registration-time failures.
///
/// # Panics
///
/// Panics when `env` is not a process-bound NIF environment.
pub fn spawn_stream<T>(
    env: Env<'_>,
    reference: Term<'_>,
    stream: Stream<T>,
) -> NifResult<ResourceArc<StreamHandle>>
where
    T: Encoder + Send + 'static,
{
    let pid = env.pid();
    let ref_env = OwnedEnv::new();
    let saved = ref_env.run(|inner| ref_env.save(reference.in_env(inner)));
    let resource = ResourceArc::new(StreamHandle {
        credits: Semaphore::new(0),
        abort: Mutex::new(None),
    });
    let handle = resource.clone();
    let task = runtime().spawn(async move {
        let mut stream = stream;
        // One message environment, cleared by every send; the reference
        // lives in `ref_env` and is copied into each message.
        let mut msg_env = OwnedEnv::new();
        loop {
            let Ok(permit) = handle.credits.acquire().await else {
                return;
            };
            permit.forget();
            if let Some(item) = stream.next().await {
                let sent = msg_env.send_and_clear(&pid, |env| {
                    let reference = ref_env.run(|re| saved.load(re).in_env(env));
                    (atoms::unibind_stream(), reference, (atoms::item(), item)).encode(env)
                });
                if sent.is_err() {
                    return;
                }
            } else {
                let _ = msg_env.send_and_clear(&pid, |env| {
                    let reference = ref_env.run(|re| saved.load(re).in_env(env));
                    (atoms::unibind_stream(), reference, atoms::done()).encode(env)
                });
                return;
            }
        }
    });
    store_and_monitor(env, &resource, &resource.abort, task.abort_handle(), pid);
    Ok(resource)
}
