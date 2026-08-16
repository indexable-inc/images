//! Conformance boundary for the unibind wasm backend. The whole crate is
//! `cfg(target_arch = "wasm32")` (ix2nix-wasm shape): the shared native
//! graph compiles an empty cdylib, and only the wasm32 unit graph in
//! packages/unibind/nix/wasm.nix builds the boundary.
#![cfg(target_arch = "wasm32")]
#![allow(
    clippy::must_use_candidate,
    reason = "these values are consumed across the JavaScript boundary, not by Rust callers"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "fallible exports surface as decoded JS exceptions; the error surface is documented in the generated index.d.ts"
)]

/// The conformance boundary (browser module `conformance`).
///
/// One `#[unibind::export(backends(wasm))]` module exercising every construct
/// the wasm backend renders: unit enums as wire strings (as arguments,
/// returns, and record fields), records in both directions (nested, under
/// `Option`, as map values, and both byte positions -- a whole argument, which
/// crosses as a `Uint8Array`, and a record field or container element, which
/// crosses as serde's `Array<number>`), error enums surfaced through the
/// `__unibind__:err:` reason channel, defaulted and optional arguments, async
/// functions with trailing-`AbortSignal` cancellation, pull streams from free
/// functions and object methods (including a stream of raw bytes and a stream
/// of records carrying 64-bit fields), a constructible resource object with
/// associated functions, a method handing back another object (sync, and
/// async-and-fallible), and the 64-bit integers, which cross as checked
/// JavaScript `number`s and never as `BigInt`.
///
/// Two things are deliberately absent, and each is a wasm fact rather than a
/// gap. There is no `blocking` export: the engine's one thread is the
/// caller's, so `unibind-backend-wasm` refuses the flag. There is no sleep
/// anywhere: `wasm32-unknown-unknown` has no tokio time driver, so every
/// async hop here is either a real executor yield (`yield_once`) or a
/// `tokio::sync::Notify` gate the suite opens itself ([`arm_pending`] /
/// [`release_pending`]), which makes "still in flight" a state the test
/// controls instead of a race against a clock.
///
/// The committed Node suite (`tests/node/conformance.test.mjs`) drives the
/// built browser package end to end; the atomic counters below exist so that
/// suite can observe Rust-side effects (dropped futures, producer progress,
/// live and closed handles) from JavaScript.
#[unibind::export(backends(wasm))]
mod conformance {
    use std::collections::HashMap;
    use std::fmt;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    /// Futures dropped before completing (see [`pending_echo`]).
    static DROPPED_MID_FLIGHT: AtomicI64 = AtomicI64::new(0);
    /// Items [`count_stream`] producers pushed into their channel so far.
    static STREAM_ITEMS_PRODUCED: AtomicI64 = AtomicI64::new(0);
    /// Items [`Session::events`] producers pushed, counted apart from the
    /// free-function streams so the method's backpressure is its own
    /// measurement.
    static SESSION_EVENTS_PRODUCED: AtomicI64 = AtomicI64::new(0);
    /// Live [`Session`] values: constructed minus dropped.
    static LIVE_SESSIONS: AtomicI64 = AtomicI64::new(0);
    /// Sessions whose `close` ran (at most once each).
    static CLOSED_SESSIONS: AtomicI64 = AtomicI64::new(0);
    /// Shells whose `close` ran (at most once each).
    static CLOSED_SHELLS: AtomicI64 = AtomicI64::new(0);
    /// Whether the pending gate is open; see [`release_pending`].
    static PENDING_RELEASED: AtomicBool = AtomicBool::new(false);

    /// One symbol occurrence in one file (a trimmed `scipql` shape). The
    /// `i64` offsets are what make this record's twin narrow field by field,
    /// and [`Facts`] inherits that through its `Vec`.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Occurrence {
        /// Symbol identifier.
        pub symbol: String,
        /// File the occurrence sits in.
        pub path: String,
        /// Byte offset where the occurrence starts.
        pub start: i64,
        /// Byte offset one past the end.
        pub end: i64,
        /// What the occurrence does at that site, e.g. `"definition"`.
        /// JavaScript spells this field its own way, which is what
        /// [`Self::role`] renders as.
        #[unibind(ts(name = "occurrenceRole"))]
        pub role: String,
    }

    /// Facts extracted from one store.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Facts {
        /// Every occurrence, in file order.
        pub occurrence: Vec<Occurrence>,
        /// Documentation keyed by symbol.
        pub docs_by_symbol: HashMap<String, String>,
        /// Raw source bytes. A record field is serde's, not the signature's,
        /// so this is an `Array<number>` and not the `Uint8Array` a whole
        /// argument crosses as (see [`blob_fixture`]).
        pub source_blob: Vec<u8>,
        /// The same bytes split up: inside a `Vec` the element is serde's
        /// too, so this nests one array level deeper. The whole of
        /// [`Self::source_blob`], chunked.
        pub blob_chunks: Vec<Vec<u8>>,
        /// The occurrence to show first, if any: a record nested under
        /// `Option`, optional in both directions.
        pub head: Option<Occurrence>,
        /// Occurrences keyed by path: records as map values.
        pub by_path: HashMap<String, Occurrence>,
    }

    /// How severe a finding is; a closed set that crosses as a union of
    /// string literals. [`Finding`] carries one.
    #[unibind::enumeration]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Severity {
        /// Routine.
        Info,
        /// Worth a look; [`escalate`] promotes [`Self::Info`] to this.
        Warning,
        /// Stop now: [`escalate`] leaves [`Self::Warning`] here.
        HardFailure,
    }

    /// A frame kind spelled `PascalCase` on the wire, the shape
    /// `MachineProgress.kind` has in the ix surface; proves `rename_all`
    /// decides the literals without a second convention.
    #[unibind::enumeration(rename_all = "PascalCase")]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FrameKind {
        /// The first frame.
        Started,
        /// The last one.
        Finished,
    }

    /// A record carrying two enumerations, each declared `String` in the
    /// twin and checked on the way in.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Finding {
        /// How bad it is: one of [`Severity`]'s literals, the worst being
        /// [`Severity::HardFailure`].
        pub severity: Severity,
        /// Which frame reported it. [`FrameKind`] is `PascalCase` on the
        /// wire, so [`FrameKind::Started`] keeps its capital.
        pub kind: FrameKind,
        /// Human text.
        pub detail: String,
    }

    /// Round-trip an enumeration: argument and return in one call.
    pub fn echo_severity(value: Severity) -> Severity {
        value
    }

    /// Round-trip an enumeration under `Option`, covering the container
    /// path.
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

    /// A record whose only adapted field is bytes: the position that reads as
    /// "nested" and is not, where serde's array of numbers is what crosses in
    /// both directions.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Blobs {
        /// A byte field on a record with no wide integer anywhere.
        pub payload: Vec<u8>,
        /// The same, optional.
        pub trailer: Option<Vec<u8>>,
    }

    /// Echo blobs through the boundary unchanged.
    pub fn echo_blobs(blobs: Blobs) -> Blobs {
        blobs
    }

    /// The byte string the suite round-trips, minted in Rust so the
    /// assertion compares against bytes JavaScript never constructed:
    /// `0x00` and `0xFF` are both unrepresentable in UTF-8 text, so any
    /// codec that decoded this as a string on the way through fails the
    /// comparison instead of passing quietly. A whole return value, so this
    /// one is the `Uint8Array` position.
    pub fn blob_fixture() -> Vec<u8> {
        vec![0x00, 0xFF, 0xFE, b'i', b'x', 0x80]
    }

    /// Everything the conformance boundary raises; [`fail_with`] mints one
    /// of each.
    #[unibind::error]
    pub enum ConformanceError {
        /// The requested store does not exist.
        #[unibind(ts(name = "StoreMissingError"))]
        StoreGone {
            /// Which store.
            name: String,
        },
        /// The query does not parse; [`Session::tail`] raises it for an
        /// empty query.
        BadQuery(String),
        /// A value fell outside the supported range, which is what
        /// [`checked_add`] refuses.
        OutOfRange {
            /// The offending sum.
            value: i64,
        },
    }

    impl fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::StoreGone { name } => write!(formatter, "store `{name}` does not exist"),
                Self::BadQuery(query) => write!(formatter, "bad query: {query}"),
                Self::OutOfRange { value } => write!(formatter, "{value} is out of range"),
            }
        }
    }

    /// A label an object method hands back: a record straight off a method,
    /// which is what puts a record type into the return position of an
    /// exported impl. See [`Session::badge`].
    #[unibind::record]
    #[derive(Clone)]
    pub struct Badge {
        /// Whatever the session is called.
        pub label: String,
    }

    /// A ledger row: the shape an SDK crosses once amounts are 64-bit.
    /// Every wide position the backend has to adapt sits in this one record
    /// -- bare, inside a `Vec`, under an `Option`, and as a map value -- so
    /// one echo exercises the twin's whole field-by-field narrowing.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Ledger {
        /// Signed balance in the smallest unit.
        pub balance: i64,
        /// Monotonic sequence number.
        pub sequence: u64,
        /// A pointer-sized count. `usize` is 32 bits on `wasm32`, which is
        /// where the narrowing stops being a no-op: see [`usize_max`].
        pub entries: usize,
        /// Per-entry deltas.
        pub deltas: Vec<i64>,
        /// An optional ceiling.
        pub ceiling: Option<i64>,
        /// Totals keyed by account.
        pub totals: HashMap<String, u64>,
    }

    /// Echo facts through the boundary unchanged: every field of
    /// [the fact record](Facts) survives.
    pub fn echo_facts(facts: Facts) -> Facts {
        facts
    }

    /// Echo a ledger unchanged; every field must survive bit for bit.
    pub fn echo_ledger(ledger: Ledger) -> Ledger {
        ledger
    }

    /// Round-trip a signed 64-bit integer.
    pub fn echo_i64(value: i64) -> i64 {
        value
    }

    /// Round-trip an unsigned 64-bit integer.
    pub fn echo_u64(value: u64) -> u64 {
        value
    }

    /// Round-trip a 32-bit unsigned integer: the narrow-width position, which
    /// `wasm-bindgen` carries natively rather than through the checked `f64`
    /// the 64-bit widths get.
    ///
    /// The consequence is a real asymmetry the suite pins: a JavaScript number
    /// that a narrow width cannot hold is *coerced* here (`ToInt32`
    /// wraparound), where the same value in an `i64` position is refused with
    /// "not a safe integer". Tightening that would flip the assertion rather
    /// than delete it.
    pub fn echo_u32(value: u32) -> u32 {
        value
    }

    /// Round-trip a pointer-sized count, which is 32 bits wide here.
    pub fn echo_usize(value: usize) -> usize {
        value
    }

    /// The signed range's own endpoints, so the suite compares against
    /// Rust's constants instead of restating them in JavaScript.
    pub fn i64_bounds() -> Vec<i64> {
        vec![i64::MIN, i64::MAX]
    }

    /// The unsigned range's top, for the same reason.
    pub fn u64_max() -> u64 {
        u64::MAX
    }

    /// `usize::MAX` as this target has it: `4294967295`, not the 64-bit
    /// value, and the exact bound [`echo_usize`] refuses one past.
    pub fn usize_max() -> usize {
        usize::MAX
    }

    /// Add, wrapping. Operands arrive as checked numbers: exact inside
    /// the double-safe range, refused outside it.
    pub fn add_i64(a: i64, #[unibind(default = 1)] b: i64) -> i64 {
        a.wrapping_add(b)
    }

    /// Sum unsigned amounts: the `Vec<u64>` argument position, which crosses
    /// as one `JsValue` through serde.
    pub fn sum_u64(values: Vec<u64>) -> u64 {
        values.into_iter().fold(0, u64::wrapping_add)
    }

    /// Echo an optional wide integer, defaulting to the top of the
    /// double-safe range when JavaScript omits it: the
    /// `Option`-with-a-default argument position.
    pub fn echo_optional_i64(
        #[unibind(default = 9_007_199_254_740_991)] value: Option<i64>,
    ) -> Option<i64> {
        value
    }

    /// Yield `count` amounts from `start`: the stream element position,
    /// where each item crosses on its own pull.
    pub fn wide_stream(start: i64, count: i64) -> unibind_runtime::UniStream<i64> {
        unibind_runtime::UniStream::new(futures::stream::iter(
            (0..count.max(0)).map(move |index| start.wrapping_add(index)),
        ))
    }

    /// Build `count` occurrences of `symbol`, all with `role`.
    pub fn make_occurrences(
        symbol: String,
        #[unibind(default = 2)] count: i64,
        role: Option<String>,
    ) -> Vec<Occurrence> {
        let role = role.unwrap_or_else(|| "reference".to_owned());
        (0..count.max(0))
            .map(|index| Occurrence {
                symbol: symbol.clone(),
                path: format!("src/file_{index}.rs"),
                start: index * 100,
                end: index * 100 + 10,
                role: role.clone(),
            })
            .collect()
    }

    /// Join `parts` with `separator`, prepending `prefix` when given.
    #[unibind(ts(name = "joinWords"))]
    pub fn join_parts(
        parts: Vec<String>,
        #[unibind(default = ", ")] separator: &str,
        prefix: Option<&str>,
    ) -> String {
        let joined = parts.join(separator);
        match prefix {
            Some(prefix) => format!("{prefix}{joined}"),
            None => joined,
        }
    }

    /// Double every byte: a whole argument and a whole return, so both ends
    /// are the `Uint8Array` position.
    pub fn double_bytes(data: Vec<u8>) -> Vec<u8> {
        data.iter().map(|byte| byte.wrapping_mul(2)).collect()
    }

    /// Round-trip an enumeration under `Option`, from an async export.
    ///
    /// The same signature as [`echo_optional_kind`] with one word changed, and
    /// the absent value does not arrive the same way: a sync wrapper hands
    /// `wasm-bindgen` the `Option` itself, which spells `None` as
    /// `undefined`, while an async one settles a `JsValue` and spells it
    /// `null`. Both are pinned in the suite, because a caller comparing
    /// against one of them is broken by the other.
    pub async fn echo_optional_kind_later(value: Option<FrameKind>) -> Option<FrameKind> {
        yield_once().await;
        value
    }

    /// Echo a path: a JavaScript string in the signature, the user's own
    /// `PathBuf` on the Rust side, and a refusal (never a lossy decode) for a
    /// path that is not valid UTF-8 on the way back.
    pub fn echo_path(path: std::path::PathBuf) -> std::path::PathBuf {
        path
    }

    /// Format `value`, exercising the three literal kinds a default can carry
    /// besides the 64-bit integers: a float, a narrower unsigned integer
    /// (which `wasm-bindgen` carries as its own Rust type rather than through
    /// a checked `f64`), and a bool.
    pub fn scale(
        value: f64,
        #[unibind(default = 0.5)] ratio: f64,
        #[unibind(default = 3)] places: u32,
        #[unibind(default = true)] rounded: bool,
    ) -> String {
        let scaled = value * ratio;
        let scaled = if rounded { scaled.round() } else { scaled };
        // `min` before the cast: a negative JavaScript number reaches a narrow
        // width wrapped rather than refused (see `echo_u32`), and a precision
        // of `u32::MAX` would abort the module instead of answering.
        let places = match usize::try_from(places.min(8)) {
            Ok(places) => places,
            Err(_) => 3,
        };
        format!("{scaled:.places$}")
    }

    /// Fail with the requested variant: `"store"` for
    /// [`ConformanceError::StoreGone`], `"query"` for
    /// [`ConformanceError::BadQuery`], anything else for
    /// [`ConformanceError::OutOfRange`].
    pub fn fail_with(variant: &str) -> Result<i64, ConformanceError> {
        match variant {
            "store" => Err(ConformanceError::StoreGone {
                name: "main".to_owned(),
            }),
            "query" => Err(ConformanceError::BadQuery("q{".to_owned())),
            _ => Err(ConformanceError::OutOfRange { value: 42 }),
        }
    }

    /// Add, rejecting sums above 1000 (a `Result` that can succeed).
    pub fn checked_add(a: i64, b: i64) -> Result<i64, ConformanceError> {
        let sum = a
            .checked_add(b)
            .ok_or(ConformanceError::OutOfRange { value: i64::MAX })?;
        if sum > 1000 {
            return Err(ConformanceError::OutOfRange { value: sum });
        }
        Ok(sum)
    }

    /// Hand control back to the executor exactly once.
    ///
    /// The stand-in for the other backends' `tokio::time::sleep(1ms)`: there
    /// is no time driver on `wasm32-unknown-unknown`, and an `async fn` that
    /// never yields would resolve inside the very
    /// `wasm_bindgen_futures::future_to_promise` call that spawned it, which
    /// proves nothing about a suspended future. This one suspends, wakes
    /// itself, and completes on the next microtask.
    async fn yield_once() {
        let mut yielded = false;
        futures::future::poll_fn(move |context| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
        .await;
    }

    /// The gate every in-flight export waits on. A `Notify` rather than a
    /// sleep, so "still in flight" is a state the suite holds open for as
    /// long as it needs instead of a race against a clock.
    fn pending_gate() -> &'static tokio::sync::Notify {
        static GATE: std::sync::OnceLock<tokio::sync::Notify> = std::sync::OnceLock::new();
        GATE.get_or_init(tokio::sync::Notify::new)
    }

    /// Wait until the gate is open. The flag is read before registering, so
    /// a [`release_pending`] that lands between the two cannot be missed.
    async fn await_release() {
        while !PENDING_RELEASED.load(Ordering::SeqCst) {
            pending_gate().notified().await;
        }
    }

    /// Close the gate: every later [`pending_echo`] stays in flight until
    /// [`release_pending`] opens it again.
    pub fn arm_pending() {
        PENDING_RELEASED.store(false, Ordering::SeqCst);
    }

    /// Open the gate, completing every call currently in flight.
    pub fn release_pending() {
        PENDING_RELEASED.store(true, Ordering::SeqCst);
        pending_gate().notify_waiters();
    }

    /// Counts a drop in [`DROPPED_MID_FLIGHT`] unless disarmed; held across
    /// [`pending_echo`]'s await so an aborted (dropped) future is observable
    /// from JavaScript. (No inherent impl: inside an exported module those
    /// are reserved for `#[unibind::object]` types.)
    struct MidFlightGuard {
        armed: bool,
    }

    impl Drop for MidFlightGuard {
        fn drop(&mut self) {
            if self.armed {
                DROPPED_MID_FLIGHT.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// Echo `value` back once the gate opens. Aborting the call drops the
    /// future while it waits, which [`dropped_mid_flight_count`] observes.
    pub async fn pending_echo(value: String) -> String {
        let mut guard = MidFlightGuard { armed: true };
        await_release().await;
        guard.armed = false;
        value
    }

    /// How many async futures were dropped before completing.
    pub fn dropped_mid_flight_count() -> i64 {
        DROPPED_MID_FLIGHT.load(Ordering::SeqCst)
    }

    /// Echo `value` after one real executor yield: the plain async success
    /// path, with a suspension in it.
    pub async fn queued_echo(value: String) -> String {
        yield_once().await;
        value
    }

    /// Yield once, then reject with the bad-query variant.
    pub async fn fail_later() -> Result<String, ConformanceError> {
        yield_once().await;
        Err(ConformanceError::BadQuery("async".to_owned()))
    }

    /// `n` items from `item`, pushed into a bounded(2) channel and tallied in
    /// `produced`, so backpressure and early close are observable from
    /// JavaScript.
    ///
    /// The producer is a detached `wasm_bindgen_futures::spawn_local` task on
    /// the same microtask queue that drives the generated promises (there is
    /// no `tokio::spawn` here: no runtime, one thread). Dropping the stream
    /// closes the channel, which is how it stops. Every counting stream here
    /// runs on this one mechanism, so a method's stream and a function's
    /// behave identically under load. No delay argument: the channel's own
    /// capacity is the throttle, since a sleep is not available.
    fn counted_stream<T: Send + 'static>(
        n: i64,
        produced: &'static AtomicI64,
        item: impl Fn(i64) -> T + 'static,
    ) -> unibind_runtime::UniStream<T> {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        wasm_bindgen_futures::spawn_local(async move {
            for value in 0..n {
                if sender.send(item(value)).await.is_err() {
                    return;
                }
                produced.fetch_add(1, Ordering::SeqCst);
            }
        });
        unibind_runtime::UniStream::new(futures::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.map(|value| (value, receiver)) },
        ))
    }

    /// Count `0..n`, observable through [`stream_items_produced`].
    pub fn count_stream(n: i64) -> unibind_runtime::UniStream<i64> {
        counted_stream(n, &STREAM_ITEMS_PRODUCED, |value| value)
    }

    /// The async composition: resolve to a stream after a suspension.
    pub async fn count_stream_later(n: i64) -> unibind_runtime::UniStream<i64> {
        yield_once().await;
        unibind_runtime::UniStream::new(futures::stream::iter(0..n.max(0)))
    }

    /// Items [`count_stream`] producers pushed so far, across every stream.
    pub fn stream_items_produced() -> i64 {
        STREAM_ITEMS_PRODUCED.load(Ordering::SeqCst)
    }

    /// A named session: a constructible resource with sync and async
    /// methods, counted by [`live_sessions`] while alive and by
    /// [`closed_sessions`] once closed.
    #[unibind::object(resource)]
    pub struct Session {
        name: String,
        open: AtomicBool,
    }

    impl Session {
        /// Open a session; rejects an empty name so the constructor's
        /// error path is exercisable from JavaScript.
        #[unibind(constructor)]
        pub fn new(name: String) -> Result<Self, ConformanceError> {
            if name.is_empty() {
                return Err(ConformanceError::BadQuery(
                    "session name must not be empty".to_owned(),
                ));
            }
            LIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
            Ok(Self {
                name,
                open: AtomicBool::new(true),
            })
        }

        /// Open a session after an async hop: the shape a constructor cannot
        /// take, since a `wasm-bindgen` constructor is synchronous. Renders
        /// as a static `Session.opened(...)` returning a promise, and there
        /// may be several such functions on one object, each keeping its own
        /// name (see [`Self::named_after`]). An empty name raises
        /// [`ConformanceError::BadQuery`].
        #[unibind(associated)]
        pub async fn opened(name: String) -> Result<Self, ConformanceError> {
            yield_once().await;
            Self::new(name)
        }

        /// A sync one beside the async one, so the two renderings are
        /// covered and a second associated function on the same object is
        /// proven.
        #[unibind(associated)]
        pub fn named_after(other: String) -> Result<Self, ConformanceError> {
            // Refuse here rather than leaning on `new`: `format!` would turn
            // an empty name into "-copy", which is a valid name, so the error
            // assertion in the conformance test would pass against a call
            // that never failed.
            if other.is_empty() {
                return Err(ConformanceError::BadQuery(
                    "session name must not be empty".to_owned(),
                ));
            }
            Self::new(format!("{other}-copy"))
        }

        /// An associated function that answers about the type rather than
        /// constructing it. Returns a [`Badge`], not the object, so the
        /// generated wrapper hands the value straight back instead of
        /// wrapping it in the class. The instance method [`Self::badge`]
        /// answers the same question.
        #[unibind(associated)]
        pub fn describe(name: String) -> Badge {
            Badge {
                label: format!("session:{name}"),
            }
        }

        /// The session's name.
        pub fn name(&self) -> String {
            self.name.clone()
        }

        /// Whether `close` has not run yet.
        pub fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        /// Answer `query` after an async hop.
        pub async fn query(&self, query: String) -> String {
            yield_once().await;
            format!("{}: {query}", self.name)
        }

        /// A record straight off a method.
        pub fn badge(&self) -> Badge {
            Badge {
                label: self.name.clone(),
            }
        }

        /// The same record, after a suspension: the async wrapper moves the
        /// call into a future, so the path lands in a different generated
        /// item than [`Self::badge`]'s.
        pub async fn badge_later(&self) -> Badge {
            yield_once().await;
            Badge {
                label: format!("{}!", self.name),
            }
        }

        /// Stream `n` events tagged with the session's name: the method twin
        /// of [`count_stream`] on the same bounded producer, counted by
        /// [`session_events_produced`].
        pub fn events(&self, n: i64) -> unibind_runtime::UniStream<String> {
            let name = self.name.clone();
            counted_stream(n, &SESSION_EVENTS_PRODUCED, move |value| {
                format!("{name}:{value}")
            })
        }

        /// Three lines for `query`, or a rejection when it is empty: the
        /// failure lands before any stream exists, so JavaScript sees a
        /// thrown error rather than a stream that ends immediately.
        pub fn tail(
            &self,
            query: String,
        ) -> Result<unibind_runtime::UniStream<String>, ConformanceError> {
            if query.is_empty() {
                return Err(ConformanceError::BadQuery("tail needs a query".to_owned()));
            }
            let name = self.name.clone();
            Ok(unibind_runtime::UniStream::new(futures::stream::iter(
                (0..3).map(move |index| format!("{name}/{query}#{index}")),
            )))
        }

        /// Resolve to a stream once the gate opens; aborting the call drops
        /// the future while it waits, which [`dropped_mid_flight_count`]
        /// observes, and no stream is ever created.
        pub async fn events_later(&self, n: i64) -> unibind_runtime::UniStream<i64> {
            let mut guard = MidFlightGuard { armed: true };
            await_release().await;
            guard.armed = false;
            unibind_runtime::UniStream::new(futures::stream::iter(0..n.max(0)))
        }

        /// Stream `n` ledger rows starting at `start`: a method whose stream
        /// element is a record carrying 64-bit fields, so the owner-scoped
        /// stream class and the record's generated twin have to compose.
        /// Each row's `balance` is `start + index`.
        pub fn ledgers(&self, start: i64, n: i64) -> unibind_runtime::UniStream<Ledger> {
            let name = self.name.clone();
            unibind_runtime::UniStream::new(futures::stream::iter((0..n.max(0)).map(
                move |index| Ledger {
                    balance: start.wrapping_add(index),
                    sequence: u64::MAX,
                    entries: count_from(index),
                    deltas: vec![index],
                    ceiling: Some(start),
                    totals: HashMap::from([(name.clone(), u64::MAX)]),
                },
            )))
        }

        /// This session's keys namespace: a method handing back another
        /// object.
        pub fn keys(&self) -> Keys {
            Keys {
                session: self.name.clone(),
            }
        }

        /// Open a shell on this session.
        ///
        /// Async, fallible, and object-returning at once, from a method on
        /// another object: the glue has to await the future, decode the error
        /// into its generated class, and still hand JavaScript the wrapper
        /// class rather than the bare `wasm-bindgen` handle. [`Self::keys`]
        /// covers only the sync half of that, and the constructor covers only
        /// the fallible half. The [`Shell`] it hands back is the generated
        /// wrapper class.
        pub async fn open_shell(&self, command: String) -> Result<Shell, ConformanceError> {
            yield_once().await;
            if command.is_empty() {
                return Err(ConformanceError::BadQuery(
                    "shell command must not be empty".to_owned(),
                ));
            }
            Ok(Shell {
                command: format!("{}/{command}", self.name),
                open: AtomicBool::new(true),
            })
        }

        /// Release the session; the generated wrapper guarantees at most one
        /// call even when JavaScript closes (or disposes) twice.
        pub async fn close(&self) {
            self.open.store(false, Ordering::SeqCst);
            CLOSED_SESSIONS.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            LIVE_SESSIONS.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// A stream index as the pointer-sized count [`Ledger::entries`] wants.
    /// Written as a match rather than `unwrap_or`, which would quietly clamp
    /// a value that did not fit.
    fn count_from(index: i64) -> usize {
        match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => usize::MAX,
        }
    }

    /// A session's keys namespace: the `client.keys().create(...)` shape,
    /// where the handle a method returns must arrive as the generated wrapper
    /// class (a bare handle would decode no errors). Not a resource: it owns
    /// nothing to close, so its class has no `close` and no leak watch.
    #[unibind::object]
    pub struct Keys {
        session: String,
    }

    impl Keys {
        /// The fully qualified name `label` would get in this session.
        pub fn create(&self, label: String) -> String {
            format!("{}/{label}", self.session)
        }

        /// Always rejects, so the returned handle's error decoding is
        /// provable from JavaScript.
        pub fn reject(&self, label: String) -> Result<String, ConformanceError> {
            Err(ConformanceError::BadQuery(label))
        }
    }

    /// A shell handle, minted only by [`Session::open_shell`]: the object
    /// half of the async-fallible-object-returning shape, and the owner of
    /// the byte stream below.
    #[unibind::object(resource)]
    pub struct Shell {
        command: String,
        open: AtomicBool,
    }

    impl Shell {
        /// The command this shell was opened with, qualified by its session.
        /// Calling it is how JavaScript proves it holds a live wrapper and
        /// not the bare `wasm-bindgen` handle.
        pub fn command(&self) -> String {
            self.command.clone()
        }

        /// Whether `close` has not run yet.
        pub fn is_open(&self) -> bool {
            self.open.load(Ordering::SeqCst)
        }

        /// `n` chunks of raw output: a byte stream off an object method,
        /// where each item is a whole stream element and so crosses as a
        /// `Uint8Array`.
        ///
        /// Every chunk opens with NUL and `0xFF`. Neither survives a UTF-8
        /// round trip, so a codec that decoded items as text anywhere on the
        /// path fails the assertion instead of passing quietly.
        pub fn output(&self, n: i64) -> unibind_runtime::UniStream<Vec<u8>> {
            let command = self.command.clone();
            unibind_runtime::UniStream::new(futures::stream::iter((0..n.max(0)).map(
                move |index| {
                    let mut chunk = vec![0x00_u8, 0xFF];
                    chunk.extend_from_slice(command.as_bytes());
                    chunk.extend_from_slice(index.to_string().as_bytes());
                    chunk
                },
            )))
        }

        /// Release the shell.
        pub async fn close(&self) {
            self.open.store(false, Ordering::SeqCst);
            CLOSED_SHELLS.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Shells closed through `close` (or disposal) so far.
    pub fn closed_shells() -> i64 {
        CLOSED_SHELLS.load(Ordering::SeqCst)
    }

    /// Items [`Session::events`] producers pushed so far, across every
    /// session.
    pub fn session_events_produced() -> i64 {
        SESSION_EVENTS_PRODUCED.load(Ordering::SeqCst)
    }

    /// Open a session from a free function (the non-constructor path);
    /// [`Session::opened`] is the associated-function path to the same
    /// thing.
    pub fn open_session(name: String) -> Session {
        LIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
        Session {
            name,
            open: AtomicBool::new(true),
        }
    }

    /// Live [`Session`] values: constructed minus dropped.
    pub fn live_sessions() -> i64 {
        LIVE_SESSIONS.load(Ordering::SeqCst)
    }

    /// Sessions closed through `close` (or disposal) so far.
    pub fn closed_sessions() -> i64 {
        CLOSED_SESSIONS.load(Ordering::SeqCst)
    }
}
