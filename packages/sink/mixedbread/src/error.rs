//! Error type for the Mixedbread sink.

use snafu::Snafu;

/// Failures from reconciling a source into a Mixedbread store.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// A source adapter failed while producing documents.
    #[snafu(display("source adapter failed: {message}"))]
    Adapter {
        /// The adapter's error, rendered.
        message: String,
    },
    /// A store operation (ensure, list, upload, delete, or index-wait) failed.
    #[snafu(display("store operation failed"))]
    Store {
        /// Underlying search-core store error.
        source: search_core::Error,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
