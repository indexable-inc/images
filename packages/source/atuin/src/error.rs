//! Error type for the atuin shell-history adapter.

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced when reading and projecting atuin history.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The history database could not be opened.
    #[snafu(display("failed to open atuin history db {}", path.display()))]
    OpenDb {
        /// Database path.
        path: PathBuf,
        /// Underlying sqlite error.
        source: rusqlite::Error,
    },

    /// The history table could not be queried.
    #[snafu(display("failed to query atuin history"))]
    Query {
        /// Underlying sqlite error.
        source: rusqlite::Error,
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
