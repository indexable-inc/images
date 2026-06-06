//! Failures from the OAuth flow and the token store.
//!
//! Every unavailable prerequisite (missing client credentials, no stored
//! token, a revoked grant, an unscoped grant) is its own variant with the
//! operator's next step in the message, so the surfaces above this crate
//! never have to guess or fall back.

use std::path::PathBuf;

use snafu::Snafu;

use crate::{CLIENT_ID_ENV, CLIENT_SECRET_ENV};

/// Failures from `google-auth`.
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

    /// The OAuth client id environment variable is unset or empty.
    #[snafu(display(
        "{CLIENT_ID_ENV} is not set; export the team Google OAuth client id \
         (see packages/google/auth/README.md)"
    ))]
    MissingClientId,

    /// The OAuth client secret environment variable is unset or empty.
    #[snafu(display(
        "{CLIENT_SECRET_ENV} is not set; export the team Google OAuth client secret \
         (see packages/google/auth/README.md)"
    ))]
    MissingClientSecret,

    /// The platform exposes no config directory to hold the token file.
    #[snafu(display("no user config directory on this platform; cannot locate the token file"))]
    NoConfigDir,

    /// No token has been stored yet.
    #[snafu(display(
        "no stored Google token at {}; run `gmail auth` (or `gcal auth`) first",
        path.display()
    ))]
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

    /// A request to the token endpoint failed to send or its body failed to
    /// decode.
    #[snafu(display("Google OAuth request failed: {source}"))]
    Http {
        /// Underlying reqwest error.
        source: reqwest::Error,
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
    #[snafu(display(
        "the stored refresh token was revoked or expired; \
         rerun `gmail auth` (or `gcal auth`)"
    ))]
    TokenRevoked,

    /// The stored grant does not carry every scope the caller needs.
    ///
    /// Surfaced when an API client asks for a scope the user has not yet
    /// consented to. The fix is to rerun the consent flow with the union of
    /// scopes; one `gmail auth` (or `gcal auth`) covers everything either
    /// binary knows about.
    #[snafu(display(
        "stored grant is missing scope `{missing}`; rerun `gmail auth` (or `gcal auth`) \
         to consent to it"
    ))]
    ScopeMissing {
        /// One scope the caller asked for that the stored grant lacks.
        missing: String,
    },

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
         rerun the `auth` command and use the URL it prints"
    ))]
    StateMismatch,

    /// The token response had no refresh token to store.
    #[snafu(display(
        "the token response carried no refresh token; revoke this app's access at \
         https://myaccount.google.com/permissions and rerun the `auth` command"
    ))]
    MissingRefreshToken,
}

/// Crate-wide result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
