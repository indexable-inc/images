//! Error type for the journald unit-log adapter.

use snafu::Snafu;

/// All failures surfaced when reading and projecting journald entries.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The `journalctl` command could not be spawned.
    #[snafu(display("failed to run journalctl"))]
    Spawn {
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The `journalctl` invocation exited non-zero.
    #[snafu(display("journalctl --since {since} failed: {stderr}"))]
    JournalctlFailed {
        /// The (normalized) timespec the command was given.
        since: String,
        /// Captured standard error.
        stderr: String,
    },

    /// A `journalctl -o json` line was not a JSON object.
    #[snafu(display("malformed journalctl json line: {detail}"))]
    Parse {
        /// What was wrong with the line.
        detail: String,
    },

    /// A built document's metadata exceeded the store's size or key limits.
    #[snafu(display("metadata limit exceeded for {external_id}"))]
    Metadata {
        /// The record whose metadata overflowed.
        external_id: String,
        /// Underlying limit error.
        source: source_meta::MetadataError,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
