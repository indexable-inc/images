//! Error type for the Claude history adapter.

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced when reading and projecting Claude transcripts.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// A directory under the history root could not be read (a missing directory
    /// is not an error; a permission or I/O fault is).
    #[snafu(display("failed to read directory {}", path.display()))]
    ReadDir {
        /// Directory that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A transcript file could not be read.
    #[snafu(display("failed to read transcript {}", path.display()))]
    ReadFile {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
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
