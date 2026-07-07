//! The JVM helpers: task outcomes across spawn/cancel/panic and shared
//! stream pulls through raw handles.

#![cfg(feature = "jvm")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use unibind_runtime::UniStream;
use unibind_runtime::jvm::{self, NextOutcome, TaskOutcome};

/// Wait for a finish callback, failing loudly instead of hanging CI.
fn recv<T>(rx: &mpsc::Receiver<T>) -> T {
    rx.recv_timeout(Duration::from_secs(10)).expect("finish fires")
}

/// Poll `condition` until it holds or a generous deadline passes.
fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn spawn_cancellable_completes() {
    let (tx, rx) = mpsc::channel();
    let task = jvm::spawn_cancellable(async { 41_u32 + 1 }, move |outcome| {
        tx.send(outcome).expect("receiver lives");
    });
    match recv(&rx) {
        TaskOutcome::Completed(value) => assert_eq!(value, 42),
        other => panic!("expected completion, got {other:?}"),
    }
    unsafe { jvm::task_free(task) };
}

/// Sets its flag when dropped, observing the cancelled future's teardown.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn spawn_cancellable_cancel_drops_the_future() {
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = DropFlag(Arc::clone(&dropped));
    let (tx, rx) = mpsc::channel();
    let task = jvm::spawn_cancellable(
        async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        },
        move |outcome| {
            tx.send(outcome).expect("receiver lives");
        },
    );
    unsafe { jvm::task_cancel(task) };
    assert!(matches!(recv(&rx), TaskOutcome::Cancelled));
    // The select drops the losing future on the worker; observe it rather
    // than assume ordering against the finish callback.
    wait_until("cancel drops the pending future", || {
        dropped.load(Ordering::SeqCst)
    });
    // Cancel is idempotent, including after completion.
    unsafe { jvm::task_cancel(task) };
    unsafe { jvm::task_free(task) };
}

#[test]
fn spawn_cancellable_reports_the_panic_text() {
    let (tx, rx) = mpsc::channel();
    let task = jvm::spawn_cancellable(
        async { panic!("boom 7") },
        move |outcome: TaskOutcome<()>| {
            tx.send(outcome).expect("receiver lives");
        },
    );
    match recv(&rx) {
        TaskOutcome::Panicked(text) => assert!(text.contains("boom 7"), "payload text: {text}"),
        other => panic!("expected a panic outcome, got {other:?}"),
    }
    unsafe { jvm::task_free(task) };
}

#[test]
fn stream_next_pulls_one_item_at_a_time() {
    let handle = jvm::stream_into_raw(UniStream::new(futures::stream::iter([1_u64, 2])));
    for expected in [Some(1), Some(2), None] {
        let (tx, rx) = mpsc::channel();
        unsafe {
            jvm::stream_next::<u64>(handle, move |outcome| {
                tx.send(outcome).expect("receiver lives");
            });
        }
        match recv(&rx) {
            NextOutcome::Item(item) => assert_eq!(item, expected),
            NextOutcome::Panicked(text) => panic!("unexpected panic: {text}"),
        }
    }
    unsafe { jvm::stream_free::<u64>(handle) };
}

#[test]
fn stream_free_during_an_in_flight_pull_is_safe() {
    let (item_tx, item_rx) = futures::channel::oneshot::channel::<u64>();
    let handle = jvm::stream_into_raw(UniStream::new(futures::stream::once(async move {
        item_rx.await.unwrap_or(0)
    })));
    let (tx, rx) = mpsc::channel();
    unsafe {
        jvm::stream_next::<u64>(handle, move |outcome| {
            tx.send(outcome).expect("receiver lives");
        });
    }
    // Free while the pull is parked on the oneshot: the pull's own clone
    // of the shared stream keeps it alive.
    unsafe { jvm::stream_free::<u64>(handle) };
    item_tx.send(7).expect("the pull holds the receiver");
    match recv(&rx) {
        NextOutcome::Item(item) => assert_eq!(item, Some(7)),
        NextOutcome::Panicked(text) => panic!("unexpected panic: {text}"),
    }
}
