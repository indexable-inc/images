//! Conformance surface for the unibind Elixir backend (phase 5, #1995).
//!
//! Every export exists so the ExUnit suite in `mix/` can prove one boundary
//! behavior from Elixir: async NIFs reply `{:unibind, ref, {:ok, _}}`, a
//! caller exiting mid-call drops the in-flight future (observable through
//! `cancelled_count`), the BEAM garbage collector runs `Drop` on resources
//! (`dropped_sessions`), and streams only produce under granted demand. The
//! statics are the observable side of behaviors that would otherwise be
//! invisible across the boundary.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`UnibindConformance`) and the OTP app (`:unibind_conformance`).
#[unibind::export(backends(ex))]
mod _unibind_conformance {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    use unibind_runtime::UniStream;

    /// A plain-data record crossing the boundary by value.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Sample {
        /// Stable identifier.
        pub id: u64,
        /// Display name.
        pub name: String,
        /// A fraction, proving floats survive the struct codec.
        pub ratio: f64,
        /// Nested list field.
        pub tags: Vec<String>,
        /// Optional field, `nil` on the Elixir side when absent.
        pub home: Option<String>,
    }

    /// Boundary failures raised by the conformance surface.
    #[unibind::error]
    #[derive(Debug)]
    pub enum ConformanceError {
        /// A deliberate failure for error-term tests.
        Deliberate { message: String },
        /// A second variant, proving variant atoms map one to one.
        Gone { message: String },
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Deliberate { message } | Self::Gone { message } => {
                    write!(formatter, "{message}")
                }
            }
        }
    }

    impl std::error::Error for ConformanceError {}

    /// Round-trip a bool.
    pub fn echo_bool(value: bool) -> bool {
        value
    }

    /// Round-trip a signed int.
    pub fn echo_int(value: i64) -> i64 {
        value
    }

    /// Round-trip an unsigned int.
    pub fn echo_uint(value: u32) -> u32 {
        value
    }

    /// Round-trip a float.
    pub fn echo_float(value: f64) -> f64 {
        value
    }

    /// Round-trip a string.
    pub fn echo_str(value: String) -> String {
        value
    }

    /// Round-trip an optional string; `nil` crosses as `None`.
    pub fn echo_option(value: Option<String>) -> Option<String> {
        value
    }

    /// Round-trip a list of ints.
    pub fn echo_vec(values: Vec<i64>) -> Vec<i64> {
        values
    }

    /// Round-trip a string-keyed map of ints.
    pub fn echo_map(values: HashMap<String, i64>) -> HashMap<String, i64> {
        values
    }

    /// Round-trip a record struct.
    pub fn echo_record(sample: Sample) -> Sample {
        sample
    }

    /// Round-trip a nested list of records.
    pub fn echo_records(samples: Vec<Sample>) -> Vec<Sample> {
        samples
    }

    /// Ok or the `:deliberate` error variant, by input.
    ///
    /// # Errors
    ///
    /// When `fail` is true; proving the `{:error, struct}` term shape is
    /// the point.
    pub fn maybe_fail(fail: bool) -> Result<i64, ConformanceError> {
        if fail {
            return Err(ConformanceError::Deliberate {
                message: "conformance deliberate failure".to_owned(),
            });
        }
        Ok(42)
    }

    /// Always the `:gone` error variant.
    ///
    /// # Errors
    ///
    /// Always; proving the second variant maps to its own atom.
    pub fn lost() -> Result<i64, ConformanceError> {
        Err(ConformanceError::Gone {
            message: "conformance gone failure".to_owned(),
        })
    }

    /// Sleep on a dirty IO scheduler; compiling and returning proves the
    /// `DirtyIo` scheduling attribute round-trips through rustler.
    #[unibind(blocking)]
    pub fn blocking_sleep_ms(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Echo through the shared tokio runtime: the plain async round-trip.
    pub async fn echo_async(value: String) -> String {
        value
    }

    /// Async Ok or the `:deliberate` error variant, by input.
    ///
    /// # Errors
    ///
    /// When `fail` is true; proving the async reply carries
    /// `{:error, struct}` too.
    pub async fn maybe_fail_async(fail: bool) -> Result<i64, ConformanceError> {
        if fail {
            return Err(ConformanceError::Deliberate {
                message: "conformance deliberate async failure".to_owned(),
            });
        }
        Ok(7)
    }

    static CANCELLED: AtomicU64 = AtomicU64::new(0);

    /// Cancellation probe: dropped while still armed (the only way the
    /// future can end other than running to completion), it bumps
    /// `CANCELLED`. A completed call disarms first, so the counter moves
    /// only when the caller's exit aborts the in-flight task. (No inherent
    /// impl: inside an exported module those are reserved for
    /// `#[unibind::object]` types.)
    struct CancelGuard {
        armed: bool,
    }

    impl Drop for CancelGuard {
        fn drop(&mut self) {
            if self.armed {
                CANCELLED.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// Sleep `ms` on the runtime holding a `CancelGuard` across the await,
    /// then resolve to `ms`. Cancelled, the guard drops armed.
    pub async fn slow(ms: u64) -> u64 {
        let mut guard = CancelGuard { armed: true };
        tokio::time::sleep(Duration::from_millis(ms)).await;
        guard.armed = false;
        ms
    }

    /// Calls of `slow` cancelled so far (armed guard drops).
    pub fn cancelled_count() -> u64 {
        CANCELLED.load(Ordering::SeqCst)
    }

    static DROPPED_SESSIONS: AtomicU64 = AtomicU64::new(0);

    /// A stateful handle proving destructor semantics: the BEAM collecting
    /// the resource (or its owning process dying) runs `Drop`, observable
    /// through `dropped_sessions`.
    #[unibind::object]
    pub struct Session {
        value: Mutex<i64>,
    }

    impl Session {
        /// Open a session holding `start`.
        #[unibind(constructor)]
        pub fn new(start: i64) -> Self {
            Self {
                value: Mutex::new(start),
            }
        }

        /// The current value.
        pub fn get(&self) -> i64 {
            *self.value.lock().expect("session mutex poisoned")
        }

        /// Add `delta`, returning the new value.
        pub fn add(&self, delta: i64) -> i64 {
            let mut value = self.value.lock().expect("session mutex poisoned");
            *value += delta;
            *value
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            DROPPED_SESSIONS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Sessions dropped so far.
    pub fn dropped_sessions() -> u64 {
        DROPPED_SESSIONS.load(Ordering::SeqCst)
    }

    /// Yield `0..n`, one item per granted credit.
    pub fn count(n: u64) -> UniStream<u64> {
        UniStream::new(futures::stream::iter(0..n))
    }

    /// Yield `n` records, proving structs cross the stream codec.
    pub fn count_samples(n: u64) -> UniStream<Sample> {
        fn sample(index: u64) -> Sample {
            Sample {
                id: index,
                name: format!("sample-{index}"),
                ratio: 0.5,
                tags: vec!["conformance".to_owned()],
                home: None,
            }
        }
        UniStream::new(futures::stream::iter((0..n).map(sample)))
    }
}
