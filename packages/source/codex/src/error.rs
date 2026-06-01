//! Error type for the Codex history adapter.

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced when reading and projecting Codex prompt history.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The history file could not be read.
    #[snafu(display("failed to read codex history {}", path.display()))]
    ReadFile {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A history line was not valid JSON.
    #[snafu(display("codex history {} line {line} is not valid JSON", path.display()))]
    ParseLine {
        /// File containing the malformed line.
        path: PathBuf,
        /// 1-based line number of the offending line.
        line: usize,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// The host name could not be resolved for record tagging.
    #[snafu(display("failed to resolve host name"))]
    HostName {
        /// Underlying OS error.
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
