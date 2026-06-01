//! The single error type for the adapter.
//!
//! Every fallible step (reading a file, parsing JSON, checking metadata) maps
//! into one [`Error`] variant so the [`SourceAdapter`](source_meta::SourceAdapter)
//! contract is satisfied with a single `Send + Sync + 'static` type.

use std::path::PathBuf;

use snafu::Snafu;

/// A failure while opening or iterating a Slack export.
///
/// Each variant carries the path or id it concerns so a failure mid-ingest
/// names the offending file or record rather than failing opaquely.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// A required file (`channels.json`, `users.json`) or a channel day file
    /// could not be read.
    #[snafu(display("failed to read {}", path.display()))]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A channel directory could not be listed.
    #[snafu(display("failed to list channel directory {}", path.display()))]
    ListDir {
        /// The directory that could not be listed.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A JSON file did not parse into the expected shape.
    #[snafu(display("failed to parse JSON in {}", path.display()))]
    Parse {
        /// The file whose contents did not parse.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// A record's flattened metadata exceeded a store limit.
    #[snafu(display("metadata rejected for {external_id}"))]
    Metadata {
        /// The record whose metadata was rejected.
        external_id: String,
        /// Underlying limit error from `search-meta`.
        source: source_meta::MetadataError,
    },
}
