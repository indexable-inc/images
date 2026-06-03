//! Error type for the Claude Code debug-log adapter.

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced when reading and projecting Claude debug logs.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The debug directory could not be read.
    #[snafu(display("failed to read Claude debug dir {}", path.display()))]
    ReadDir {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A debug log file could not be read.
    #[snafu(display("failed to read Claude debug log {}", path.display()))]
    ReadFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
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
