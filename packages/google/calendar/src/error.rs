//! Failures from the Calendar client and the OAuth flow.
//!
//! Every unavailable prerequisite (missing client credentials, no stored
//! token, a revoked grant) is its own variant with the operator's next step in
//! the message, so the surfaces above this crate never have to guess or fall
//! back.

use std::path::PathBuf;

use snafu::Snafu;

use crate::auth::{CLIENT_ID_ENV, CLIENT_SECRET_ENV};

/// Failures from the Google Calendar client.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
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

    /// The OAuth client id environment variable is unset or empty.
    #[snafu(display(
        "{CLIENT_ID_ENV} is not set; export the team Google OAuth client id \
         (see packages/google/calendar/README.md)"
    ))]
    MissingClientId,

    /// The OAuth client secret environment variable is unset or empty.
    #[snafu(display(
        "{CLIENT_SECRET_ENV} is not set; export the team Google OAuth client secret \
         (see packages/google/calendar/README.md)"
    ))]
    MissingClientSecret,

    /// The platform exposes no config directory to hold the token file.
    #[snafu(display("no user config directory on this platform; cannot locate the token file"))]
    NoConfigDir,

    /// No token has been stored yet.
    #[snafu(display("no stored Google token at {}; run `gcal auth` first", path.display()))]
    NoToken {
        /// Expected token file location.
        path: PathBuf,
    },

    /// The token file exists but could not be read.
    #[snafu(display("failed to read token file {}: {source}", path.display()))]
    ReadToken {
        /// Token file location.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The token file could not be written.
    #[snafu(display("failed to write token file {}: {source}", path.display()))]
    WriteToken {
        /// Token file location.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// The token file holds something other than a stored token.
    #[snafu(display("token file {} is not a valid stored token: {source}", path.display()))]
    ParseToken {
        /// Token file location.
        path: PathBuf,
        /// Underlying JSON error.
        source: serde_json::Error,
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

    /// The OAuth token endpoint rejected an exchange or refresh.
    #[snafu(display("Google OAuth token endpoint returned {status}: {body}"))]
    TokenExchange {
        /// HTTP status code.
        status: u16,
        /// Response body.
        body: String,
    },

    /// The stored refresh token no longer works.
    #[snafu(display("the stored refresh token was revoked or expired; run `gcal auth` again"))]
    TokenRevoked,

    /// The consent flow could not bind or accept on the loopback listener.
    #[snafu(display("OAuth loopback listener failed: {source}"))]
    Listen {
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// A pasted redirect could not be parsed as a URL.
    #[snafu(display("could not parse the OAuth redirect URL: {input:?}"))]
    RedirectParse {
        /// The rejected input.
        input: String,
    },

    /// Google reported a consent error (for example `access_denied`).
    #[snafu(display("Google denied the consent request: {reason}"))]
    ConsentDenied {
        /// The `error` parameter from the redirect.
        reason: String,
    },

    /// The redirect carried no authorization code.
    #[snafu(display("the OAuth redirect carried no authorization code"))]
    MissingCode,

    /// The redirect's `state` does not belong to this consent attempt.
    #[snafu(display(
        "the OAuth redirect state did not match this consent attempt; \
         rerun `gcal auth` and use the URL it prints"
    ))]
    StateMismatch,

    /// The token response had no refresh token to store.
    #[snafu(display(
        "the token response carried no refresh token; revoke this app's access at \
         https://myaccount.google.com/permissions and run `gcal auth` again"
    ))]
    MissingRefreshToken,
}

/// Crate-wide result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
