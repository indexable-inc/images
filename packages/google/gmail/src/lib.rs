//! Typed client for the [Gmail v1 API](https://developers.google.com/gmail/api/reference/rest):
//! search, read, send, modify, draft, and manage labels on the user's
//! mailbox.
//!
//! This crate owns the domain logic for the email capability (#599, #644):
//! the HTTP client, the wire types, error mapping, and the MIME builder
//! that turns an [`OutgoingMessage`] into RFC 5322 bytes for `messages.send`
//! and `drafts.create`. OAuth is shared with the calendar crate through
//! [`google_auth`]. The user-facing surfaces stay thin per RFC 0003: the
//! `gmail` CLI (`packages/google/gmail/cli`) shapes arguments, and the
//! mail tools in the Rust MCP server (`packages/google/mcp`) call the
//! same client in-process.
//!
//! Scopes used: `gmail.modify` (read, label, archive, trash, drafts) plus
//! `gmail.send`. Both must be present on the stored grant or
//! [`Authenticator::access_token`] returns [`google_auth::Error::ScopeMissing`].

mod attachments;
mod drafts;
mod error;
mod labels;
mod messages;
mod mime;
mod model;
mod threads;

pub use drafts::{DraftMessageStub, DraftStub};
pub use error::{Error, Result};
pub use google_auth::scopes::{ALL_KNOWN as ALL_KNOWN_SCOPES, GMAIL_MODIFY, GMAIL_SEND};
pub use google_auth::{
    AuthCode, Authenticator, ClientSecrets, PendingConsent, StoredToken, TokenStore, begin_consent,
};
pub use messages::{InvalidMessageFormat, LABEL_INBOX, LABEL_UNREAD, MessageFormat, MessageStub};
pub use model::{
    Attachment, Draft, Header, Label, Message, MessagePart, MessagePartBody, MessageQuery,
    OutgoingMessage, Thread,
};
pub use threads::ThreadStub;

use serde::Deserialize;
use snafu::ResultExt as _;
use url::Url;

use crate::error::{ApiSnafu, BadBaseUrlSnafu, BuildClientSnafu, HttpSnafu, NotABaseUrlSnafu};

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://gmail.googleapis.com/gmail/v1";

/// User-id placeholder that maps to the authenticated user.
pub const USER_ME: &str = "me";

/// Page-size ceiling for `messages.list` / `threads.list`. The API caps
/// `maxResults` at 500 per page; larger queries follow `nextPageToken`.
pub(crate) const MAX_PAGE_SIZE: usize = 500;

/// The Gmail / Google API error envelope:
/// `{"error": {"code": …, "message": …, "status": …, "errors": [...]}}`.
#[derive(Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Gmail API client over an [`Authenticator`].
pub struct Client {
    http: reqwest::Client,
    auth: Authenticator,
    base_url: Url,
    user_id: String,
}

impl Client {
    /// A client against the real API, acting as the authenticated user
    /// (`USER_ME`).
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be built.
    pub fn new(auth: Authenticator) -> Result<Self> {
        Self::with_base_url(auth, DEFAULT_BASE_URL)
    }

    /// A client against a different base URL (tests).
    ///
    /// # Errors
    /// Returns an error if the base URL does not parse, cannot hold path
    /// segments, or the HTTP client cannot be built.
    pub fn with_base_url(auth: Authenticator, base_url: &str) -> Result<Self> {
        let parsed = Url::parse(base_url).context(BadBaseUrlSnafu { input: base_url })?;
        snafu::ensure!(
            !parsed.cannot_be_a_base(),
            NotABaseUrlSnafu { input: base_url }
        );
        Ok(Self {
            http: reqwest::Client::builder().build().context(BuildClientSnafu)?,
            auth,
            base_url: parsed,
            user_id: USER_ME.to_owned(),
        })
    }

    /// Act as a different mailbox (delegated access, tests). Domain
    /// administrators can grant access to other users' mailboxes; this
    /// switches the `users/{userId}/...` path segment.
    #[must_use]
    pub fn as_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = user_id.into();
        self
    }

    /// Build a URL under `users/{user_id}/<resource>` for the configured
    /// mailbox.
    pub(crate) fn user_url<I>(&self, segments: I) -> Url
    where
        I: IntoIterator<Item: AsRef<str>>,
    {
        let mut url = self.base_url.clone();
        {
            let mut writer = url
                .path_segments_mut()
                .expect("with_base_url rejects cannot-be-a-base URLs");
            writer.push("users").push(&self.user_id);
            for segment in segments {
                writer.push(segment.as_ref());
            }
        }
        url
    }

    /// A bearer-authenticated `GET` builder against `url`.
    pub(crate) async fn get(&self, url: Url) -> Result<reqwest::RequestBuilder> {
        let token = self.auth.access_token().await?;
        Ok(self.http.get(url).bearer_auth(token))
    }

    /// A bearer-authenticated `POST` builder against `url`.
    pub(crate) async fn post(&self, url: Url) -> Result<reqwest::RequestBuilder> {
        let token = self.auth.access_token().await?;
        Ok(self.http.post(url).bearer_auth(token))
    }

    /// A bearer-authenticated `PUT` builder against `url`.
    pub(crate) async fn put(&self, url: Url) -> Result<reqwest::RequestBuilder> {
        let token = self.auth.access_token().await?;
        Ok(self.http.put(url).bearer_auth(token))
    }

    /// A bearer-authenticated `DELETE` builder against `url`.
    pub(crate) async fn delete(&self, url: Url) -> Result<reqwest::RequestBuilder> {
        let token = self.auth.access_token().await?;
        Ok(self.http.delete(url).bearer_auth(token))
    }
}

/// Map a non-success status onto [`Error::Api`] with the message from
/// Google's error envelope; pass a success through.
pub(crate) async fn check_status(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    ApiSnafu {
        status: status.as_u16(),
        message: api_message(&body),
    }
    .fail()
}

/// Decode a checked JSON response.
pub(crate) async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T> {
    check_status(response)
        .await?
        .json()
        .await
        .context(HttpSnafu)
}

/// Send a request, check its status, and discard the body. Used for the
/// bodyless `trash`/`untrash`/`delete` paths.
pub(crate) async fn send_no_body(builder: reqwest::RequestBuilder) -> Result<()> {
    let response = builder.send().await.context(HttpSnafu)?;
    check_status(response).await?;
    Ok(())
}

/// The human message from a Google error body, or the (truncated) raw body
/// when the envelope is absent.
fn api_message(body: &str) -> String {
    serde_json::from_str::<ApiErrorBody>(body).map_or_else(
        |_| {
            let trimmed = body.trim();
            let mut message: String = trimmed.chars().take(500).collect();
            if message.len() < trimmed.len() {
                message.push('…');
            }
            message
        },
        |envelope| envelope.error.message,
    )
}

#[cfg(test)]
mod tests {
    use super::api_message;

    #[test]
    fn api_message_prefers_the_error_envelope() {
        let body =
            r#"{"error":{"code":403,"message":"Insufficient Permission","status":"PERMISSION_DENIED"}}"#;
        assert_eq!(api_message(body), "Insufficient Permission");
    }

    #[test]
    fn api_message_falls_back_to_the_raw_body() {
        assert_eq!(api_message(" gmail blew up "), "gmail blew up");
    }
}
