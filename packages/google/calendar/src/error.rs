//! Failures from the Calendar client. Auth-flow failures live in
//! [`google_auth::Error`]; this enum carries them through transparently
//! via the [`Error::Auth`] variant so callers only handle one error type.

use snafu::Snafu;

/// Failures from the Google Calendar client.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// An auth-flow failure: no stored token, a revoked grant, a missing
    /// scope, or a transport error against the OAuth token endpoint.
    #[snafu(display("{source}"))]
    Auth {
        /// Underlying auth error.
        source: google_auth::Error,
    },

    /// The HTTP client could not be constructed.
    #[snafu(display("failed to build HTTP client: {source}"))]
    BuildClient {
        /// Underlying reqwest error.
        source: reqwest::Error,
    },

    /// The base URL override is not a valid URL.
    #[snafu(display("invalid Calendar API base URL {input:?}: {source}"))]
    BadBaseUrl {
        /// The rejected input.
        input: String,
        /// Underlying parse error.
        source: url::ParseError,
    },

    /// The base URL override cannot hold path segments (for example `data:`).
    #[snafu(display("Calendar API base URL {input:?} cannot hold path segments; use http(s)"))]
    NotABaseUrl {
        /// The rejected input.
        input: String,
    },

    /// A request failed to send or its body failed to decode.
    #[snafu(display("Google Calendar request failed: {source}"))]
    Http {
        /// Underlying reqwest error.
        source: reqwest::Error,
    },

    /// The Calendar API returned a non-success status.
    #[snafu(display("Google Calendar API returned {status}: {message}"))]
    Api {
        /// HTTP status code.
        status: u16,
        /// Message extracted from the error body.
        message: String,
    },
}

impl From<google_auth::Error> for Error {
    fn from(source: google_auth::Error) -> Self {
        Self::Auth { source }
    }
}

/// Crate-wide result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
