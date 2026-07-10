//! sqlmerge: a git merge driver for `SQLite` database files.
//!
//! Three-way merge via the `SQLite` session extension. The public surface is
//! [`merge::merge`] plus the typed [`error::MergeError`]; the `sqlmerge` binary
//! is a thin argv wrapper over it. See the crate README for git wiring and
//! semantics.
//!
//! Built by Claude Code.

pub mod config;
pub mod error;
pub mod merge;
pub mod schema;

pub use config::{ConfigError, PolicyConfig};
pub use error::{MergeError, Result};
pub use merge::{ConflictPolicy, merge};
