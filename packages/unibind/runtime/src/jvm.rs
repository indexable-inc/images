//! JVM async helpers the generated glue calls.
//!
//! Generated `extern "C"` exports never name `tokio` directly: they spawn
//! through [`spawn_cancellable`] and pull streams through [`stream_next`],
//! so this module is the single indirection point pinning the runtime
//! semantics: one shared multi-thread runtime, cancellation drops the
//! in-flight future, and panics cross as text.

use std::ffi::c_void;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

use futures::FutureExt as _;

use crate::shared::{panic_text, SharedStream};
use crate::UniStream;

/// The process-wide runtime every exported async call and stream pull runs
/// on.
///
/// Started lazily, with all drivers enabled, and never shut down: task and
/// stream handles held by Java can outlive any scope on the Rust side, so
/// the runtime's lifetime is the process's.
///
/// # Panics
///
/// Panics when the runtime cannot start (worker thread spawn failure).
#[must_use]
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("unibind: the shared tokio runtime failed to start")
    })
}

/// How a task spawned by [`spawn_cancellable`] ended.
#[derive(Debug)]
pub enum TaskOutcome<T> {
    /// The future ran to completion with this value.
    Completed(T),
    /// The future panicked; the payload rendered as text.
    Panicked(String),
    /// [`task_cancel`] won: the future was dropped mid-flight.
    Cancelled,
}

/// The state behind an opaque task handle: cancellation is a one-way
/// notification into the spawned task.
struct CancelState {
    notify: tokio::sync::Notify,
}

/// Spawn `fut` on the shared runtime; `finish` fires exactly once with the
/// outcome. Returns an opaque handle for [`task_cancel`] / [`task_free`].
///
/// Cancellation races completion inside a `select!`: whichever side wins
/// calls `finish`, and losing the race to a cancel drops the user future
/// mid-flight (that drop is the cancellation contract).
#[must_use]
pub fn spawn_cancellable<F, T>(
    fut: F,
    finish: impl FnOnce(TaskOutcome<T>) + Send + 'static,
) -> *mut c_void
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(CancelState {
        notify: tokio::sync::Notify::new(),
    });
    let task_state = Arc::clone(&state);
    runtime().spawn(async move {
        let caught = AssertUnwindSafe(fut).catch_unwind();
        tokio::select! {
            outcome = caught => match outcome {
                Ok(value) => finish(TaskOutcome::Completed(value)),
                Err(payload) => finish(TaskOutcome::Panicked(panic_text(payload.as_ref()))),
            },
            // `notified()` consumes the permit `notify_one` stores, so a
            // cancel that lands before this select is first polled still
            // wins the race.
            () = task_state.notify.notified() => finish(TaskOutcome::Cancelled),
        }
    });
    Arc::into_raw(state).cast_mut().cast::<c_void>()
}

/// Request cancellation of a task spawned by [`spawn_cancellable`].
///
/// Idempotent, and a no-op once the task completed; null is ignored.
///
/// # Safety
///
/// `task` must be null or a handle returned by [`spawn_cancellable`] that
/// has not yet been passed to [`task_free`].
pub unsafe fn task_cancel(task: *mut c_void) {
    if task.is_null() {
        return;
    }
    let state = unsafe { &*task.cast_const().cast::<CancelState>() };
    state.notify.notify_one();
}

/// Release a task handle returned by [`spawn_cancellable`].
///
/// Null is ignored. The spawned task holds its own reference, so freeing
/// while the task is still running is safe.
///
/// # Safety
///
/// `task` must be null or a handle returned by [`spawn_cancellable`],
/// passed here exactly once and never used afterwards.
pub unsafe fn task_free(task: *mut c_void) {
    if task.is_null() {
        return;
    }
    drop(unsafe { Arc::from_raw(task.cast_const().cast::<CancelState>()) });
}

/// How one stream pull issued by [`stream_next`] ended.
#[derive(Debug)]
pub enum NextOutcome<T> {
    /// The next item, or `None` once the stream is exhausted.
    Item(Option<T>),
    /// The stream panicked while producing; the payload as text.
    Panicked(String),
}

/// Box a stream behind an opaque handle for [`stream_next`] /
/// [`stream_free`].
#[must_use]
pub fn stream_into_raw<T: Send + 'static>(stream: UniStream<T>) -> *mut c_void {
    Box::into_raw(Box::new(SharedStream::new(stream))).cast::<c_void>()
}

/// Pull one item from a stream handle; `finish` fires exactly once on the
/// shared runtime.
///
/// The shared stream is cloned out of the handle before the pull is
/// spawned, so a [`stream_free`] that lands while the pull is in flight
/// cannot invalidate it.
///
/// # Safety
///
/// `handle` must come from [`stream_into_raw::<T>`](stream_into_raw) with
/// the same `T`, and [`stream_free`] must not have been called before this
/// call returns.
pub unsafe fn stream_next<T: Send + 'static>(
    handle: *mut c_void,
    finish: impl FnOnce(NextOutcome<T>) + Send + 'static,
) {
    let shared = SharedStream::clone(unsafe { &*handle.cast_const().cast::<SharedStream<T>>() });
    runtime().spawn(async move {
        let caught = AssertUnwindSafe(shared.next()).catch_unwind();
        match caught.await {
            Ok(item) => finish(NextOutcome::Item(item)),
            Err(payload) => finish(NextOutcome::Panicked(panic_text(payload.as_ref()))),
        }
    });
}

/// Release a stream handle returned by [`stream_into_raw`].
///
/// # Safety
///
/// `handle` must come from [`stream_into_raw::<T>`](stream_into_raw) with
/// the same `T`, passed here exactly once and never used afterwards.
pub unsafe fn stream_free<T: Send + 'static>(handle: *mut c_void) {
    drop(unsafe { Box::from_raw(handle.cast::<SharedStream<T>>()) });
}
