//! Conformance surface for the unibind Python backend (phase 2, #1992).
//!
//! Every export here exists so `runner.py` can prove one boundary behavior
//! from Python: asyncio cancellation drops the Rust future, streams are
//! pull-based (including a stream of raw bytes off an object method),
//! resources close deterministically (and warn when leaked), an async
//! fallible method mints another object as a live wrapper class,
//! `&[u8]` crosses zero-copy, and `blocking` releases the GIL. The globals
//! are the observable side of behaviors that would otherwise be invisible
//! across the boundary.

/// The exported boundary. The module name names the `PyInit_` symbol, so
/// the built cdylib imports as `_conformance`.
// `backends(py)`: a whole-workspace build unifies unibind's backend
// features across consumers (the ts and ex conformance crates enable `ts`
// and `ex`), so pin this crate's glue to the backend whose runtime deps it
// declares.
#[unibind::export(backends(py))]
mod _conformance {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use unibind_runtime::UniStream;

    /// A plain-data record crossing the boundary by value.
    ///
    /// [`echo_record`] hands one back unchanged, so a coordinate that
    /// survives the trip proves the by-value path in both directions.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Point {
        /// Horizontal coordinate, paired with [`Self::y`].
        pub x: f64,
        /// Vertical coordinate.
        pub y: f64,
    }

    /// How severe a conformance probe's finding is; a closed set whose
    /// members cross as `StrEnum` values, one to a [`Finding`].
    #[unibind::enumeration]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Severity {
        /// Routine.
        Info,
        /// Worth a look. [`escalate`] promotes it to [`Self::HardFailure`].
        Warning,
        /// Stop now.
        HardFailure,
    }

    /// A frame kind spelled `PascalCase` on the wire, the shape
    /// `MachineProgress.kind` has in the ix surface; proves `rename_all`
    /// reaches the generated members without a second convention.
    #[unibind::enumeration(rename_all = "PascalCase")]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FrameKind {
        /// The first frame.
        Started,
        /// The last one.
        Finished,
    }

    /// A record carrying two enumerations, so the field path is covered as
    /// well as the argument and return paths.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Finding {
        /// How bad it is.
        pub severity: Severity,
        /// Which frame reported it.
        pub kind: FrameKind,
        /// Human text.
        pub detail: String,
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

    /// Round-trip an enumeration: argument and return in one call.
    pub fn echo_severity(value: Severity) -> Severity {
        value
    }

    /// Round-trip an enumeration under `Option`, so the container path is
    /// covered too.
    pub fn echo_optional_kind(value: Option<FrameKind>) -> Option<FrameKind> {
        value
    }

    /// Round-trip a record whose fields are enumerations.
    pub fn echo_finding(finding: Finding) -> Finding {
        finding
    }

    /// The next severity up, so the Rust side is observably matching on the
    /// variant rather than passing a string through.
    pub fn escalate(value: Severity) -> Severity {
        match value {
            Severity::Info => Severity::Warning,
            Severity::Warning | Severity::HardFailure => Severity::HardFailure,
        }
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

        /// Open a gate after an async hop: the shape `__new__` cannot take,
        /// since a Python constructor is synchronous. Renders as a
        /// `@staticmethod` returning a coroutine, and one object may carry
        /// several of these, each keeping its own name.
        ///
        /// # Errors
        ///
        /// Raises [`ConformanceError::Deliberate`] on an empty label, the same
        /// refusal [`Self::named_after`] makes.
        #[unibind(associated)]
        pub async fn opened(label: String) -> Result<Self, ConformanceError> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Self::new(label)
        }

        /// A sync one beside the async one, so both renderings and a
        /// second associated function on one object are covered.
        #[unibind(associated)]
        pub fn named_after(other: String) -> Result<Self, ConformanceError> {
            // Refuse here rather than leaning on `new`: `format!` would
            // turn an empty label into "-copy", which is a valid label, so
            // the error assertion in the runner would pass against a call
            // that never failed.
            if other.is_empty() {
                return Err(ConformanceError::Deliberate {
                    message: "gate label must not be empty".to_owned(),
                });
            }
            Self::new(format!("{other}-copy"))
        }

        /// An associated function that answers about the type rather
        /// than constructing it: a plain `@staticmethod` returning a
        /// string, not the object.
        #[unibind(associated)]
        pub fn describe(label: String) -> String {
            format!("gate:{label}")
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

        /// Open a shell on this gate.
        ///
        /// Async, fallible, and object-returning at once, from a method on
        /// another object: the wrapper has to await the future, map the
        /// error into the exception hierarchy, and route the success value
        /// through `Shell`'s glue class. Every other object here is minted
        /// by a constructor or a sync call, so nothing else reaches that
        /// combination. Its bytes come back through [`Shell::output`].
        ///
        /// # Errors
        ///
        /// Rejects an empty command, so the error arm of the same shape is
        /// exercisable from Python.
        pub async fn open_shell(&self, command: String) -> Result<Shell, ConformanceError> {
            tokio::time::sleep(Duration::from_millis(1)).await;
            if command.is_empty() {
                return Err(ConformanceError::Deliberate {
                    message: "shell command must not be empty".to_owned(),
                });
            }
            Ok(Shell {
                command: format!("{}/{command}", self.label),
                open: AtomicBool::new(true),
            })
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

    static CLOSED_SHELLS: AtomicU64 = AtomicU64::new(0);

    /// A shell handle, minted only by [`Gate::open_shell`]: the object half
    /// of the async-fallible-object-returning shape, and the owner of the
    /// byte stream below.
    #[unibind::object(resource)]
    pub struct Shell {
        command: String,
        open: AtomicBool,
    }

    impl Shell {
        /// The command this shell was opened with, qualified by its gate.
        /// Calling it is how Python proves it holds a live wrapper and not
        /// an opaque handle.
        pub fn command(&self) -> String {
            self.command.clone()
        }

        /// Whether `close` has not run yet.
        pub fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        /// `n` chunks of raw output: a byte stream off an object method.
        ///
        /// Every chunk opens with NUL and `0xFF`. Neither survives a UTF-8
        /// round trip, so a codec that decoded items as text anywhere on
        /// the path fails the assertion instead of passing quietly. What
        /// follows the prefix is the command qualified by the label it was
        /// opened under; see [the opening handle](Gate).
        pub fn output(&self, n: u64) -> UniStream<Vec<u8>> {
            let command = self.command.clone();
            UniStream::new(futures::stream::iter((0..n).map(move |index| {
                let mut chunk = vec![0x00_u8, 0xFF];
                chunk.extend_from_slice(command.as_bytes());
                chunk.extend_from_slice(index.to_string().as_bytes());
                chunk
            })))
        }

        /// Release the shell.
        pub async fn close(&self) {
            self.open.store(false, Ordering::SeqCst);
            CLOSED_SHELLS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Shells closed through `close` so far.
    pub fn closed_shells() -> u64 {
        CLOSED_SHELLS.load(Ordering::SeqCst)
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
