//! Conformance surface for the unibind Python backend (phase 2, #1992).
//!
//! Every export here exists so `runner.py` can prove one boundary behavior
//! from Python: asyncio cancellation drops the Rust future, streams are
//! pull-based, resources close deterministically (and warn when leaked),
//! `&[u8]` crosses zero-copy, and `blocking` releases the GIL. The globals
//! are the observable side of behaviors that would otherwise be invisible
//! across the boundary.

/// The exported boundary. The module name names the `PyInit_` symbol, so
/// the built cdylib imports as `_conformance`.
#[unibind::export]
mod _conformance {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use unibind_runtime::UniStream;

    /// A plain-data record crossing the boundary by value.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Point {
        /// Horizontal coordinate.
        pub x: f64,
        /// Vertical coordinate.
        pub y: f64,
    }

    /// Boundary failures raised by the conformance surface.
    #[unibind::error(py(base = "ValueError"))]
    #[derive(Debug)]
    pub enum ConformanceError {
        /// A deliberate failure for exception-mapping tests.
        Deliberate { message: String },
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Deliberate { message } => write!(formatter, "{message}"),
            }
        }
    }

    impl std::error::Error for ConformanceError {}

    /// Round-trip a bool.
    pub fn echo_bool(value: bool) -> bool {
        value
    }

    /// Round-trip an int.
    pub fn echo_int(value: i64) -> i64 {
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

    /// Round-trip bytes; the argument view is copied into an owned return.
    pub fn echo_bytes(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    /// Round-trip an optional int.
    pub fn echo_option(value: Option<i64>) -> Option<i64> {
        value
    }

    /// Round-trip a list of ints.
    pub fn echo_vec(values: Vec<i64>) -> Vec<i64> {
        values
    }

    /// Round-trip a string-keyed map of floats.
    pub fn echo_map(values: HashMap<String, f64>) -> HashMap<String, f64> {
        values
    }

    /// Round-trip a record.
    pub fn echo_record(point: Point) -> Point {
        point
    }

    /// Add with a defaulted second operand, proving `#[unibind(default)]`.
    pub fn add_with_default(value: i64, #[unibind(default = 32)] delta: i64) -> i64 {
        value + delta
    }

    /// Raise the generated `ValueError` subclass.
    ///
    /// # Errors
    ///
    /// Always: proving the enum maps onto the exception hierarchy is the
    /// point.
    pub fn throw_value_error() -> Result<(), ConformanceError> {
        Err(ConformanceError::Deliberate {
            message: "conformance deliberate failure".to_owned(),
        })
    }

    /// Address of the first byte as Rust sees the buffer; Python compares
    /// it against `ctypes.addressof` to prove no copy happened.
    pub fn buffer_addr(data: &[u8]) -> usize {
        data.as_ptr() as usize
    }

    /// Sleep on the calling thread with the GIL released; two Python
    /// threads overlapping is the observable proof of `blocking`.
    #[unibind(blocking)]
    pub fn blocking_sleep_ms(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Wrapping byte sum, computed off the GIL.
    #[unibind(blocking)]
    pub fn checksum(data: &[u8]) -> u64 {
        data.iter()
            .fold(0u64, |acc, byte| acc.wrapping_add(u64::from(*byte)))
    }

    static LIVE: AtomicU64 = AtomicU64::new(0);
    static DROPPED: AtomicU64 = AtomicU64::new(0);

    /// Cancellation probe: the only way `DROPPED` moves is this guard being
    /// dropped, which is exactly what asyncio cancellation must cause on
    /// the Rust future holding it. (No inherent impl: inside an exported
    /// module those are reserved for `#[unibind::object]` types.)
    struct DropGuard;

    impl Drop for DropGuard {
        fn drop(&mut self) {
            LIVE.fetch_sub(1, Ordering::SeqCst);
            DROPPED.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Hold a `DropGuard` across a sleep that never ends on its own; only
    /// cancellation from Python can release it.
    pub async fn hold_guard_forever() {
        LIVE.fetch_add(1, Ordering::SeqCst);
        let _guard = DropGuard;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }

    /// Guards currently alive.
    pub fn live_guards() -> u64 {
        LIVE.load(Ordering::SeqCst)
    }

    /// Guards dropped so far.
    pub fn dropped_guards() -> u64 {
        DROPPED.load(Ordering::SeqCst)
    }

    /// Sleep `ms`, then resolve to `value`: the plain async round-trip.
    pub async fn sleep_ms_then(ms: u64, value: i64) -> i64 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        value
    }

    static PRODUCED: AtomicU64 = AtomicU64::new(0);

    /// Yield `0..n`, bumping `PRODUCED` once per yielded item. The stream is
    /// pull-based, so the counter tracks consumer demand: after three
    /// `__anext__` calls it must read (about) three, not `n`.
    pub fn counting_stream(n: u64) -> UniStream<u64> {
        UniStream::new(futures::stream::unfold(0u64, move |state| async move {
            if state >= n {
                return None;
            }
            PRODUCED.fetch_add(1, Ordering::SeqCst);
            Some((state, state + 1))
        }))
    }

    /// Items produced across every `counting_stream` so far.
    pub fn produced_count() -> u64 {
        PRODUCED.load(Ordering::SeqCst)
    }

    /// A stream of records behind an async fn, covering the
    /// `async fn -> UniStream<record>` composition.
    pub async fn record_stream(n: u64) -> UniStream<Point> {
        // Conformance indices stay far below 2^53, so the lossy-cast lint
        // does not apply in spirit; f64::from does not take u64.
        #[allow(clippy::cast_precision_loss)]
        fn point(index: u64) -> Point {
            Point {
                x: index as f64,
                y: -(index as f64),
            }
        }
        UniStream::new(futures::stream::iter((0..n).map(point)))
    }

    static CLOSED_GATES: AtomicU64 = AtomicU64::new(0);

    /// A stateful handle proving resource semantics: `close` is observable
    /// and idempotent at the boundary, and leaking without close warns.
    #[unibind::object(resource)]
    pub struct Gate {
        label: String,
        open: AtomicBool,
    }

    impl Gate {
        /// Open a gate.
        ///
        /// # Errors
        ///
        /// Rejects an empty label, so the constructor's error path is
        /// exercisable from Python.
        #[unibind(constructor)]
        pub fn new(label: String) -> Result<Self, ConformanceError> {
            if label.is_empty() {
                return Err(ConformanceError::Deliberate {
                    message: "gate label must not be empty".to_owned(),
                });
            }
            Ok(Self {
                label,
                open: AtomicBool::new(true),
            })
        }

        /// The label the gate was opened with.
        pub fn label(&self) -> String {
            self.label.clone()
        }

        /// Whether `close` has not run yet.
        pub fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        /// Await `ms` milliseconds on the runtime, then echo it back.
        pub async fn ping(&self, ms: u64) -> u64 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            ms
        }

        /// Release the gate. The generated wrapper guarantees at most one
        /// call even when Python awaits `close()` twice, which is what
        /// `closed_gates` verifies.
        ///
        /// # Errors
        ///
        /// Never in practice; the `Result` proves fallible close crosses
        /// the boundary.
        pub async fn close(&self) -> Result<(), ConformanceError> {
            self.open.store(false, Ordering::SeqCst);
            CLOSED_GATES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Gates closed through `close` so far.
    pub fn closed_gates() -> u64 {
        CLOSED_GATES.load(Ordering::SeqCst)
    }

    /// Panic synchronously.
    ///
    /// # Panics
    ///
    /// Always: proving panics surface as Python exceptions without killing
    /// the interpreter is the point.
    pub fn panic_sync() {
        panic!("unibind conformance: deliberate sync panic");
    }

    /// Panic inside the spawned future, one timer poll in, so the panic
    /// happens on the runtime rather than at call time.
    ///
    /// # Panics
    ///
    /// Always.
    pub async fn panic_async() {
        tokio::time::sleep(Duration::from_millis(1)).await;
        panic!("unibind conformance: deliberate async panic");
    }
}
