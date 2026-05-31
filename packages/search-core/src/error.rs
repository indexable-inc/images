//! Error type for the crate. Every fallible boundary returns [`Error`] so the
//! binary can render one operator-facing message and the library never panics
//! on expected failures (missing files, HTTP errors, malformed responses).

use std::path::PathBuf;

use snafu::Snafu;

/// All failures surfaced by the library.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The directory walker failed partway through enumeration.
    #[snafu(display("failed to walk {}: {source}", root.display()))]
    Walk {
        /// Root the walk started from.
        root: PathBuf,
        /// Underlying walker error.
        source: repo_walker::WalkError,
    },

    /// A file selected for indexing could not be read.
    #[snafu(display("failed to read {}: {source}", path.display()))]
    ReadFile {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A file's metadata (size, mtime) could not be queried.
    #[snafu(display("failed to stat {}: {source}", path.display()))]
    Stat {
        /// File that could not be stat'd.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// No user cache directory is available to store the manifest.
    #[snafu(display("could not determine a cache directory for the manifest"))]
    NoCacheDir,

    /// The manifest database's parent cache directory could not be created.
    #[snafu(display("failed to create cache directory {}: {source}", path.display()))]
    CreateCacheDir {
        /// Directory that could not be created.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The manifest database could not be opened.
    #[snafu(display("failed to open manifest database {}: {source}", path.display()))]
    OpenDb {
        /// Database path.
        path: PathBuf,
        /// Underlying `SQLite` error.
        source: rusqlite::Error,
    },

    /// A manifest database query or update failed.
    #[snafu(display("manifest database error: {source}"))]
    Db {
        /// Underlying `SQLite` error.
        source: rusqlite::Error,
    },

    /// A sync would touch more files than the configured ceiling allows.
    #[snafu(display(
        "sync would upload {count} files, over the limit of {max}; nothing was uploaded"
    ))]
    TooManyFiles {
        /// Number of files the sync wanted to upload.
        count: usize,
        /// Configured maximum.
        max: usize,
    },

    /// A file's metadata could not be encoded for upload.
    #[snafu(display("failed to encode file metadata: {source}"))]
    EncodeMetadata {
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// A stored or built document had missing or malformed metadata.
    #[snafu(display("document {external_id} has invalid metadata: missing or bad key {key:?}"))]
    InvalidMetadata {
        /// The record whose metadata was invalid.
        external_id: String,
        /// The metadata key that was missing or unparseable.
        key: &'static str,
    },

    /// A document's metadata exceeded the store's size or key limits.
    #[snafu(display("metadata limit exceeded: {source}"))]
    MetadataLimit {
        /// Underlying limit error.
        source: search_meta::MetadataError,
    },

    /// A source adapter failed while producing a document.
    #[snafu(display("source adapter failed: {message}"))]
    Adapter {
        /// The adapter's error, rendered.
        message: String,
    },

    /// The storage backend (Mixedbread client) returned an error.
    #[snafu(display("storage backend error: {source}"))]
    Backend {
        /// Underlying client error.
        source: mixedbread::Error,
    },

    /// A grep pattern was not a valid regular expression.
    #[snafu(display("invalid grep pattern {pattern:?}: {source}"))]
    InvalidPattern {
        /// The pattern that failed to compile.
        pattern: String,
        /// Underlying regex compilation error.
        source: regex::Error,
    },
}

/// Convenient result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
