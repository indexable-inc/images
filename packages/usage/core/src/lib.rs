//! Local usage telemetry for ix-distributed tools (index#3802).
//!
//! The hot path is [`spool::append`]: one `O_APPEND` write, no locks, no
//! `SQLite`, so wrapped tools stay fast under arbitrary parallelism. A
//! flock-singleton compactor ([`store::compact`]) folds spool records into
//! `SQLite` (WAL), the queryable source of truth for humans and agents alike.
//!
//! Privacy invariant: upload payloads are built only by
//! [`payload::build_report`], which reads only the `counts` and `meta`
//! tables. Raw error records (argv, cwd) have no code path off the machine.

pub mod consent;
pub mod paths;
pub mod payload;
pub mod spool;
pub mod store;
