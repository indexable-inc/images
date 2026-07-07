//! Conformance engine for the unibind Rust client backend (phase 4,
//! issue #1994).
//!
//! One `#[unibind::export]` module exercising the whole `rs` surface:
//! records (map, option, vec, nesting, and a deliberately awkward field
//! order), an error enum, sync functions, and async functions. Built as a
//! cdylib; the checked-in `unibind-conformance-client` crate is generated
//! from this file and the conformance check runs the consumer binary
//! against the built library.
//!
//! The async pair is the cancellation proof: `hang_until_dropped` never
//! completes but holds a guard whose `Drop` flips a process-global witness,
//! so a client that drops the returned future can observe (via
//! `cancel_witnessed`) that the engine-side future really died.

#[unibind::export(backends(rs))]
mod conformance {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use unibind_stream::UniStream;

    /// A sample record. `flag` sits first on purpose: Rust would pack the
    /// struct tighter reordered, which exercises the generated mirror's
    /// layout-assert opt-out.
    #[unibind::record]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Sample {
        /// Deliberately awkward leading bool.
        pub flag: bool,
        /// Identifier.
        pub id: u64,
        /// Display name.
        pub name: String,
        /// Optional note.
        pub note: Option<String>,
        /// Plain values.
        pub values: Vec<i64>,
        /// Keyed weights.
        pub weights: HashMap<String, i64>,
        /// A nested record.
        pub inner: Inner,
    }

    /// The nested half of [`Sample`].
    #[unibind::record]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Inner {
        /// A label.
        pub label: String,
        /// A ratio.
        pub ratio: f64,
    }

    /// Everything the conformance boundary raises.
    #[unibind::error]
    #[derive(Debug)]
    pub enum ConformanceError {
        /// The store is gone.
        StoreGone {
            /// What was being looked for.
            message: String,
        },
        /// The input is invalid.
        Invalid(String),
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::StoreGone { message } => write!(formatter, "store gone: {message}"),
                Self::Invalid(message) => write!(formatter, "invalid input: {message}"),
            }
        }
    }

    /// Round-trip a record through the boundary unchanged.
    pub fn echo_record(sample: Sample) -> Sample {
        sample
    }

    /// Sum the values.
    pub fn sum(values: Vec<i64>) -> i64 {
        values.iter().sum()
    }

    /// Fail with the variant selected by `kind` (0 and 1); anything else
    /// succeeds with the kind echoed back.
    ///
    /// # Errors
    ///
    /// [`ConformanceError::StoreGone`] for 0, [`ConformanceError::Invalid`]
    /// for 1.
    pub fn fail(kind: u32) -> Result<u64, ConformanceError> {
        match kind {
            0 => Err(ConformanceError::StoreGone {
                message: "kind 0".to_owned(),
            }),
            1 => Err(ConformanceError::Invalid("kind 1".to_owned())),
            other => Ok(u64::from(other)),
        }
    }

    /// Double `x` after yielding once, so the waker crosses the boundary
    /// (the future wakes itself and completes on the second poll).
    pub async fn delayed_double(x: i64) -> i64 {
        YieldOnce { yielded: false }.await;
        x * 2
    }

    /// Never completes; holds a guard whose `Drop` flips the cancellation
    /// witness. Dropping the returned future on the client side must run
    /// that guard through the ABI vtable.
    pub async fn hang_until_dropped() -> u64 {
        let _guard = CancelWitnessGuard;
        PendingForever.await;
        // Unreachable: `PendingForever` never resolves; the future only
        // ever ends by being dropped.
        0
    }

    /// Count `0..limit`, returning `Pending` (with a wake) between items,
    /// so every element exercises the cross-ABI waker path.
    pub fn count_to(limit: u64) -> UniStream<u64> {
        UniStream::new(CountTo {
            next: 0,
            limit,
            ready: false,
        })
    }

    /// Clear the cancellation witness before a new observation.
    pub fn reset_cancel_witness() {
        CANCEL_WITNESS.store(false, Ordering::SeqCst);
    }

    /// Whether a `hang_until_dropped` future has been dropped (cancelled)
    /// since the last reset.
    pub fn cancel_witnessed() -> bool {
        CANCEL_WITNESS.load(Ordering::SeqCst)
    }

    /// Set by [`CancelWitnessGuard::drop`]; process-global so the client
    /// can observe the cancellation across the ABI.
    static CANCEL_WITNESS: AtomicBool = AtomicBool::new(false);

    /// Flips [`CANCEL_WITNESS`] when the future holding it is dropped.
    struct CancelWitnessGuard;

    impl Drop for CancelWitnessGuard {
        fn drop(&mut self) {
            CANCEL_WITNESS.store(true, Ordering::SeqCst);
        }
    }

    /// Pending on the first poll (waking itself so the executor returns),
    /// ready on the second: the minimal future that exercises the
    /// cross-ABI waker path.
    struct YieldOnce {
        yielded: bool,
    }

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// The body of [`count_to`]: pending-with-wake between items.
    struct CountTo {
        next: u64,
        limit: u64,
        ready: bool,
    }

    impl futures_core::Stream for CountTo {
        type Item = u64;

        fn poll_next(
            mut self: Pin<&mut Self>,
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

    /// Never ready: the body of [`hang_until_dropped`].
    struct PendingForever;

    impl Future for PendingForever {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<()> {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future as _;
    use std::task::{Context, Poll, Waker};

    use crate::conformance;

    #[test]
    fn fail_maps_kinds_to_variants() {
        assert!(matches!(
            conformance::fail(0),
            Err(conformance::ConformanceError::StoreGone { .. })
        ));
        assert!(matches!(
            conformance::fail(1),
            Err(conformance::ConformanceError::Invalid(_))
        ));
        assert_eq!(conformance::fail(7).expect("non-error kind"), 7);
    }

    #[test]
    fn delayed_double_yields_once_then_completes() {
        let mut future = Box::pin(conformance::delayed_double(21));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert_eq!(future.as_mut().poll(&mut context), Poll::Ready(42));
    }

    #[test]
    fn dropping_the_hanging_future_sets_the_witness() {
        conformance::reset_cancel_witness();
        let mut future = Box::pin(conformance::hang_until_dropped());
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(!conformance::cancel_witnessed(), "no cancel before drop");
        drop(future);
        assert!(conformance::cancel_witnessed(), "drop runs the guard");
    }

    #[test]
    fn count_to_yields_the_full_sequence_with_pendings() {
        let mut stream = ::std::pin::pin!(conformance::count_to(3));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let mut seen = Vec::new();
        let mut pendings = 0;
        loop {
            match futures_core::Stream::poll_next(stream.as_mut(), &mut context) {
                Poll::Ready(Some(item)) => seen.push(item),
                Poll::Ready(None) => break,
                Poll::Pending => pendings += 1,
            }
        }
        assert_eq!(seen, [0, 1, 2]);
        assert_eq!(pendings, 3, "one yield per item");
    }

    /// The exported future types must satisfy `DynFuture`'s bounds and the
    /// stream must satisfy `DynStream`'s (`Send` only: `UniStream`'s inner
    /// box is deliberately not `Sync`). The hanging future stays unpolled
    /// here, so dropping it never creates (or fires) the cancellation guard
    /// and the witness test cannot race.
    #[test]
    fn engine_futures_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>(_value: T) {}
        fn assert_send<T: Send>(_value: T) {}
        assert_send_sync(conformance::delayed_double(1));
        assert_send_sync(conformance::hang_until_dropped());
        assert_send(conformance::count_to(1));
    }
}
