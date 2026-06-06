//! Failures from the Gmail client. Auth-flow failures live in
//! [`google_auth::Error`]; this enum wraps them through transparently via
//! the [`Error::Auth`] variant so callers only handle one error type.

use snafu::Snafu;

/// Failures from the Google Gmail client.
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
    #[snafu(display("invalid Gmail API base URL {input:?}: {source}"))]
    BadBaseUrl {
        /// The rejected input.
        input: String,
        /// Underlying parse error.
        source: url::ParseError,
    },

    /// The base URL override cannot hold path segments (for example `data:`).
    #[snafu(display("Gmail API base URL {input:?} cannot hold path segments; use http(s)"))]
    NotABaseUrl {
        /// The rejected input.
        input: String,
    },

    /// A request failed to send or its body failed to decode.
    #[snafu(display("Gmail request failed: {source}"))]
    Http {
        /// Underlying reqwest error.
        source: reqwest::Error,
    },

    /// The Gmail API returned a non-success status.
    #[snafu(display("Gmail API returned {status}: {message}"))]
    Api {
        /// HTTP status code.
        status: u16,
        /// Message extracted from the error body.
        message: String,
    },

    /// A base64url field in the wire response could not be decoded.
    ///
    /// Gmail returns message bodies, attachment payloads, and the `raw`
    /// field as URL-safe base64 (no padding). A decode failure means the
    /// API answered with malformed bytes (a Google bug); the field name
    /// names the offender.
    #[snafu(display("failed to decode base64url field `{field}`: {source}"))]
    Base64 {
        /// Which wire field rejected decoding.
        field: &'static str,
        /// Underlying base64 error.
        source: base64::DecodeError,
    },

    /// A composed outgoing message would violate RFC 5322 framing.
    ///
    /// Surfaced when a header value contains a bare newline or a control
    /// character that would let a user-supplied subject smuggle additional
    /// headers into the message. The fix is at the caller: strip the
    /// offending bytes from the field.
    #[snafu(display("header `{header}` contains a forbidden character"))]
    UnsafeHeader {
        /// Which header carried the bad value.
        header: &'static str,
    },
}

impl From<google_auth::Error> for Error {
    fn from(source: google_auth::Error) -> Self {
        Self::Auth { source }
    }
}

/// Crate-wide result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
