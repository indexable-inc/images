//! Conformance addon for the unibind TypeScript backend.
//!
//! One `#[unibind::export]` module exercising every construct the ts
//! backend renders: records, error enums, defaulted and optional
//! arguments, async functions with cancellation, pull streams (sync and
//! async producers), and a constructible resource object. It mirrors the
//! shapes of the shared Python conformance surface
//! (`packages/unibind/conformance`), which cannot compile under the ts
//! backend itself because its py-shaped types (u64, usize) are
//! BigInt-territory the ts backend still rejects. The committed Node
//! suite (`tests/node/conformance.test.mjs`) drives the built addon end
//! to end; the atomic counters below exist so that suite can observe
//! Rust-side effects (dropped futures, producer progress, live and closed
//! handles) from JavaScript.

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
    /// Live [`Session`] values: constructed minus dropped.
    static LIVE_SESSIONS: AtomicI64 = AtomicI64::new(0);
    /// Sessions whose `close` ran (at most once each).
    static CLOSED_SESSIONS: AtomicI64 = AtomicI64::new(0);

    /// One symbol occurrence in one file (a trimmed `scipql` shape; the
    /// offsets are `i64` because `usize` would need BigInt).
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

    /// Echo facts through the boundary unchanged.
    pub fn echo_facts(facts: Facts) -> Facts {
        facts
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

    /// Count `0..n` into a bounded(2) channel with `delay_ms` between
    /// items, so backpressure and early close are observable through
    /// [`stream_items_produced`]. The producer is a real detached task:
    /// dropping the stream closes the channel, which is how it stops.
    pub fn count_stream(
        n: i64,
        #[unibind(default = 0)] delay_ms: i64,
    ) -> unibind_runtime::UniStream<i64> {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let delay = millis_duration(delay_ms);
        // Detach the producer; napi's spawn targets the same tokio runtime
        // that drives the generated async wrappers.
        drop(napi::bindgen_prelude::spawn(async move {
            for value in 0..n {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if sender.send(value).await.is_err() {
                    return;
                }
                STREAM_ITEMS_PRODUCED.fetch_add(1, Ordering::SeqCst);
            }
        }));
        unibind_runtime::UniStream::new(futures::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.map(|value| (value, receiver)) },
        ))
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
                return Err(ConformanceError::BadQuery("session name must not be empty".to_owned()));
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
