//! Error type for the git commit-history adapter.

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced when reading and projecting git history.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The `git` command could not be spawned.
    #[snafu(display("failed to run git in {}", repo.display()))]
    Spawn {
        /// Repository the command targeted.
        repo: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The `git log` invocation exited non-zero.
    #[snafu(display("git log failed in {}: {stderr}", repo.display()))]
    GitFailed {
        /// Repository the command targeted.
        repo: PathBuf,
        /// Captured standard error.
        stderr: String,
    },

    /// A `git log` record did not have the expected field layout.
    #[snafu(display("malformed git log record: {detail}"))]
    Parse {
        /// What was wrong with the record.
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
