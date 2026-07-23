//! Error type for the Mixedbread sink.

use snafu::Snafu;

/// Failures from reconciling a source into a Mixedbread store.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// A store operation (ensure, list, upload, delete, or index-wait) failed.
    #[snafu(display("store operation failed"))]
    Store {
        /// Underlying search-core store error.
        source: search_core::Error,
    },
    /// A `source == X`-scoped listing returned records claiming another source
    /// (or none): the backend did not apply the scope. Acting on such a listing
    /// is refused outright — its delete set would span the whole store.
    #[snafu(display(
        "listing scoped to {scope} leaked {count} foreign record(s) (e.g. {example}); \
         refusing to act on an unscoped listing"
    ))]
    ScopeLeak {
        /// The source the listing was scoped to.
        scope: String,
        /// How many returned records claimed another source.
        count: usize,
        /// One leaked record's `external_id`, for the log.
        example: String,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
