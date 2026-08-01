//! Conformance addon for the unibind TypeScript backend.
//!
//! One `#[unibind::export]` module exercising every construct the ts
//! backend renders: records (including a record under `Option` and records
//! as map values), error enums, defaulted and optional arguments, async
//! functions with cancellation, pull streams from both free functions and
//! object methods (sync and async producers), a constructible resource
//! object, a method handing back another object, and the 64-bit integers
//! that only a JavaScript `bigint` can carry exactly. It mirrors the shapes
//! of the shared Python conformance surface
//! (`packages/unibind/conformance`). The committed Node suite
//! (`tests/node/conformance.test.mjs`) drives the built addon end to end;
//! the atomic counters below exist so that suite can observe Rust-side
//! effects (dropped futures, producer progress, live and closed handles)
//! from JavaScript.

#![allow(
    clippy::must_use_candidate,
    reason = "these values are consumed across the JS boundary, not by Rust callers"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "fallible exports surface as decoded JS exceptions; the error surface is documented in the generated index.d.ts"
)]

/// The conformance boundary (JS module `conformance`).
#[unibind::export(backends(ts))]
mod conformance {
    use std::collections::HashMap;
    use std::fmt;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::time::Duration;

    /// Futures dropped before completing (see [`sleep_echo`]).
    static DROPPED_MID_FLIGHT: AtomicI64 = AtomicI64::new(0);
    /// Items `count_stream` producers pushed into their channel so far.
    static STREAM_ITEMS_PRODUCED: AtomicI64 = AtomicI64::new(0);
    /// Items [`Session::events`] producers pushed, counted apart from the
    /// free-function streams so the method's backpressure is its own
    /// measurement.
    static SESSION_EVENTS_PRODUCED: AtomicI64 = AtomicI64::new(0);
    /// Live [`Session`] values: constructed minus dropped.
    static LIVE_SESSIONS: AtomicI64 = AtomicI64::new(0);
    /// Sessions whose `close` ran (at most once each).
    static CLOSED_SESSIONS: AtomicI64 = AtomicI64::new(0);

    /// One symbol occurrence in one file (a trimmed `scipql` shape). The
    /// `i64` offsets make this a record the glue crosses through a mirror
    /// struct, and `Facts` below inherits that through its `Vec`.
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
        /// Raw source bytes (nested bytes stay `Array<number>`).
        pub source_blob: Vec<u8>,
        /// The occurrence to show first, if any: a record nested under
        /// `Option`, optional in both directions.
        pub head: Option<Occurrence>,
        /// Occurrences keyed by path: records as map values.
        pub by_path: HashMap<String, Occurrence>,
    }

    /// Everything the conformance boundary raises.
    #[unibind::error]
    pub enum ConformanceError {
        /// The requested store does not exist.
        #[unibind(ts(name = "StoreMissingError"), py(name = "StoreMissing"))]
        StoreGone { name: String },
        /// The query does not parse.
        BadQuery(String),
        /// A value fell outside the supported range.
        OutOfRange { value: i64 },
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

    /// A label an object method hands back.
    ///
    /// Deliberately narrow: no 64-bit field, so it crosses as the user's
    /// own `#[napi(object)]` struct and not through a generated mirror.
    /// That is the whole point of it -- see [`Session::badge`].
    #[unibind::record]
    #[derive(Clone)]
    pub struct Badge {
        /// Whatever the session is called.
        pub label: String,
    }

    /// A ledger row: the shape an SDK crosses once amounts are 64-bit.
    /// Every wide position the backend has to adapt sits in this one record
    /// -- bare, inside a `Vec`, under an `Option`, and as a map value -- so
    /// one echo exercises the mirror's whole field-by-field narrowing.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Ledger {
        /// Signed balance in the smallest unit.
        pub balance: i64,
        /// Monotonic sequence number.
        pub sequence: u64,
        /// A pointer-sized count.
        pub entries: usize,
        /// Per-entry deltas.
        pub deltas: Vec<i64>,
        /// An optional ceiling.
        pub ceiling: Option<i64>,
        /// Totals keyed by account.
        pub totals: HashMap<String, u64>,
    }

    /// Echo facts through the boundary unchanged.
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

    /// Round-trip a pointer-sized count.
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

    /// Add, wrapping. A caller working past 2^53 proves both operands
    /// arrived exact, which a `number` boundary could not deliver.
    pub fn add_i64(a: i64, #[unibind(default = 1)] b: i64) -> i64 {
        a.wrapping_add(b)
    }

    /// Sum unsigned amounts: the `Vec<u64>` argument position.
    pub fn sum_u64(values: Vec<u64>) -> u64 {
        values.into_iter().fold(0, u64::wrapping_add)
    }

    /// Echo an optional wide integer, defaulting to one past 2^53 when
    /// JavaScript omits it: the `Option`-with-a-default argument position.
    pub fn echo_optional_i64(
        #[unibind(default = 9_007_199_254_740_993)] value: Option<i64>,
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

    /// Double every byte (top-level bytes cross as `Buffer`).
    pub fn double_bytes(data: Vec<u8>) -> Vec<u8> {
        data.iter().map(|byte| byte.wrapping_mul(2)).collect()
    }

    /// Fail with the requested variant: `"store"`, `"query"`, or anything
    /// else for the out-of-range variant.
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

    /// JavaScript sleeps arrive as `i64` milliseconds; negative values
    /// clamp to zero rather than erroring, since the suite only probes
    /// timing.
    fn millis_duration(millis: i64) -> Duration {
        Duration::from_millis(millis.max(0).unsigned_abs())
    }

    /// Counts a drop in [`DROPPED_MID_FLIGHT`] unless disarmed; held
    /// across [`sleep_echo`]'s await so an aborted (dropped) future is
    /// observable from JavaScript. (No inherent impl: inside an exported
    /// module those are reserved for `#[unibind::object]` types.)
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

    /// Echo `value` back after `millis` milliseconds. Aborting the call
    /// drops the future mid-sleep, which [`dropped_mid_flight_count`]
    /// observes.
    pub async fn sleep_echo(value: String, millis: i64) -> String {
        let mut guard = MidFlightGuard { armed: true };
        tokio::time::sleep(millis_duration(millis)).await;
        guard.armed = false;
        value
    }

    /// How many async futures were dropped before completing.
    pub fn dropped_mid_flight_count() -> i64 {
        DROPPED_MID_FLIGHT.load(Ordering::SeqCst)
    }

    /// Sleep, then reject with the bad-query variant.
    pub async fn sleep_fail(millis: i64) -> Result<String, ConformanceError> {
        tokio::time::sleep(millis_duration(millis)).await;
        Err(ConformanceError::BadQuery("async".to_owned()))
    }

    /// `n` items from `item`, pushed `delay_ms` apart into a bounded(2)
    /// channel and tallied in `produced`, so backpressure and early close
    /// are observable from JavaScript. The producer is a real detached
    /// task: dropping the stream closes the channel, which is how it
    /// stops. Every counting stream here runs on this one mechanism, so a
    /// method's stream and a function's behave identically under load.
    fn counted_stream<T: Send + 'static>(
        n: i64,
        delay_ms: i64,
        produced: &'static AtomicI64,
        item: impl Fn(i64) -> T + Send + 'static,
    ) -> unibind_runtime::UniStream<T> {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let delay = millis_duration(delay_ms);
        // Detach the producer; napi's spawn targets the same tokio runtime
        // that drives the generated async wrappers.
        drop(napi::bindgen_prelude::spawn(async move {
            for value in 0..n {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if sender.send(item(value)).await.is_err() {
                    return;
                }
                produced.fetch_add(1, Ordering::SeqCst);
            }
        }));
        unibind_runtime::UniStream::new(futures::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.map(|value| (value, receiver)) },
        ))
    }

    /// Count `0..n`, observable through [`stream_items_produced`].
    pub fn count_stream(
        n: i64,
        #[unibind(default = 0)] delay_ms: i64,
    ) -> unibind_runtime::UniStream<i64> {
        counted_stream(n, delay_ms, &STREAM_ITEMS_PRODUCED, |value| value)
    }

    /// The async composition: resolve to a stream after an await.
    pub async fn count_stream_later(n: i64) -> unibind_runtime::UniStream<i64> {
        tokio::time::sleep(Duration::from_millis(1)).await;
        unibind_runtime::UniStream::new(futures::stream::iter(0..n.max(0)))
    }

    /// Items `count_stream` producers pushed so far, across every stream.
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
            tokio::time::sleep(Duration::from_millis(1)).await;
            format!("{}: {query}", self.name)
        }

        /// A record straight off a method.
        ///
        /// [`Badge`] carries no 64-bit field, so it crosses as the user's
        /// own struct rather than through a mirror -- which is what puts a
        /// path into the exported module inside the impl napi-derive
        /// relocates. Until an object method returned one, every method
        /// here answered with a primitive, an object handle or a stream,
        /// and none of those spell that path. The first real adopter's
        /// surface did, and could not build.
        pub fn badge(&self) -> Badge {
            Badge {
                label: self.name.clone(),
            }
        }

        /// The same record, after an await: the async wrapper moves the
        /// call into a future, so the path lands in a different generated
        /// item than [`Self::badge`]'s.
        pub async fn badge_later(&self) -> Badge {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Badge {
                label: format!("{}!", self.name),
            }
        }

        /// Stream `n` events tagged with the session's name: the method
        /// twin of [`count_stream`] on the same bounded producer, counted
        /// by [`session_events_produced`].
        pub fn events(
            &self,
            n: i64,
            #[unibind(default = 0)] delay_ms: i64,
        ) -> unibind_runtime::UniStream<String> {
            let name = self.name.clone();
            counted_stream(n, delay_ms, &SESSION_EVENTS_PRODUCED, move |value| {
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

        /// Resolve to a stream after `millis`; aborting the call drops the
        /// future mid-sleep, which [`dropped_mid_flight_count`] observes,
        /// and no stream is ever created.
        pub async fn events_later(&self, n: i64, millis: i64) -> unibind_runtime::UniStream<i64> {
            let mut guard = MidFlightGuard { armed: true };
            tokio::time::sleep(millis_duration(millis)).await;
            guard.armed = false;
            unibind_runtime::UniStream::new(futures::stream::iter(0..n.max(0)))
        }

        /// Stream `n` ledger rows starting at `start`: a method whose
        /// stream element is a record carrying 64-bit fields, so the
        /// owner-scoped stream class and the record's generated mirror
        /// have to compose. Each row's `balance` is `start + index`, past
        /// 2^53 when the caller asks for it.
        pub fn ledgers(&self, start: i64, n: i64) -> unibind_runtime::UniStream<Ledger> {
            let name = self.name.clone();
            unibind_runtime::UniStream::new(futures::stream::iter((0..n.max(0)).map(
                move |index| Ledger {
                    balance: start.wrapping_add(index),
                    sequence: u64::MAX,
                    entries: usize::try_from(index).unwrap_or(usize::MAX),
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

        /// Release the session; the generated wrapper guarantees at most
        /// one call even when JavaScript closes (or disposes) twice.
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

    /// A session's keys namespace: the `client.keys().create(...)` shape,
    /// where the handle a method returns must arrive as the generated
    /// wrapper class (a bare native handle would decode no errors).
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

    /// Items [`Session::events`] producers pushed so far, across every
    /// session.
    pub fn session_events_produced() -> i64 {
        SESSION_EVENTS_PRODUCED.load(Ordering::SeqCst)
    }

    /// Open a session from a free function (the non-constructor path).
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
