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

    /// The db file exists but has no `history` table: atuin has not run (or not
    /// finished its first-run migration) for this account, so there is nothing
    /// to index. Distinct from [`Error::Query`] so a caller can treat it as a
    /// soft, non-fatal skip rather than a genuine read failure.
    #[snafu(display("atuin history db {} is uninitialized (no history table)", path.display()))]
    UninitializedDb {
        /// Database path.
        path: PathBuf,
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

impl Error {
    /// Whether this error is the benign "db exists but atuin never initialized
    /// it" case ([`Error::UninitializedDb`]). The fleet indexer reads many
    /// accounts' history; an account that has an atuin db file but no `history`
    /// table yet (atuin has not run there) is a soft skip, not a run failure.
    #[must_use]
    pub const fn is_uninitialized(&self) -> bool {
        matches!(self, Self::UninitializedDb { .. })
    }
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
