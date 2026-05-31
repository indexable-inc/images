//! `indexbench`: a metric-centric continuous-benchmarking framework.
//!
//! The core abstraction is a [`Metric`](schema::Metric) — a named number with a
//! unit and a direction — not a time. Time is just one metric alongside peak
//! RSS, allocation counts, and any custom value a consumer reports. Three
//! sources feed metrics into one [`Run`](schema::Run):
//!
//! - The [micro harness](micro) times a Rust closure (distributional
//!   `wall_clock`) and, behind a counting allocator, reports a deterministic
//!   `allocations` count.
//! - The [macro harness](macro_harness) runs an external command N times,
//!   reading `wall_clock` and `max_rss` from `wait4`/`getrusage`, and ingests
//!   the command's own `@bench name=… value=… …` lines as custom metrics.
//! - A consumer declares both kinds in a [`BenchSuite`](suite::BenchSuite).
//!
//! Runs are appended to a durable [history store](store) (an orphan
//! `bench-history` git branch by default), and the [comparator](compare)
//! classifies each metric against the previous run on the same machine:
//! distributional metrics get a Mann-Whitney U test plus an effect-size
//! threshold, deterministic metrics get an exact compare. The
//! [reporter](report) renders the result as a table or JSON, and the CLI exits
//! non-zero on any regression so it can gate CI.
//!
//! Deterministic alloc-count metrics are reproducible and so are suitable as
//! `nix flake check`s; timing and RSS are sandbox-sensitive and belong in the
//! `apps.bench` perf job. The crate supports both paths through the same schema.

pub mod compare;
pub mod error;
pub mod macro_harness;
pub mod micro;
pub mod report;
pub mod run;
pub mod schema;
pub mod store;
pub mod suite;

pub use error::{Error, Result};
pub use schema::{Metric, Run, machine_id};
