//! Typed client for the [Google Calendar v3 events API](https://developers.google.com/workspace/calendar/api/v3/reference/events):
//! list, get, create, and cancel events.
//!
//! This crate owns the domain logic for the calendar capability (#643): the
//! HTTP client, the wire types, OAuth, and error mapping. The user-facing
//! surfaces stay thin per RFC 0003: the `gcal` CLI
//! (`packages/google/calendar/cli`) shapes arguments, and the ix-mcp calendar
//! tools (`packages/mcp`) run that CLI with `--json`.
//!
//! The `auth` module owns the shared Google grant: its consent now covers Gmail
//! (`gmail.modify`, `gmail.send`) alongside `calendar.events`, and
//! `gcal print-access-token` hands a current token to the bundled Python
//! `google_auth` helper so notebook cells can drive Gmail and Calendar through
//! the official client. When more Google surfaces land, this `auth` module is
//! the part to graduate into a shared `packages/google/auth` crate (#644).

pub mod auth;
mod error;
mod model;

pub use auth::{
    AccessToken, AuthCode, Authenticator, ClientSecrets, EVENTS_SCOPE, GMAIL_MODIFY_SCOPE,
    GMAIL_SEND_SCOPE, PendingConsent, StoredToken, TokenStore, begin_consent,
};
pub use error::{Error, Result};
pub use model::{
    Attendee, AttendeeDraft, Event, EventDraft, EventQuery, EventTime, Person, SendUpdates,
};

use serde::Deserialize;
use snafu::ResultExt as _;
use url::Url;

use crate::error::{ApiSnafu, BadBaseUrlSnafu, BuildClientSnafu, HttpSnafu, NotABaseUrlSnafu};

/// Default API base URL.
pub const DEFAULT_BASE_URL: &str = "https://www.googleapis.com/calendar/v3";

/// The calendar most callers mean: the authenticated user's primary calendar.
pub const PRIMARY_CALENDAR: &str = "primary";

/// Page-size ceiling for `events.list`; the API caps `maxResults` at 250 per
/// page, so larger queries follow `nextPageToken`.
const MAX_PAGE_SIZE: usize = 250;

/// One page of `events.list`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsPage {
    #[serde(default)]
    items: Vec<Event>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// The Google API error envelope: `{"error": {"code": …, "message": …}}`.
#[derive(Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Calendar API client over an [`Authenticator`].
pub struct Client {
    http: reqwest::Client,
    auth: Authenticator,
    base_url: Url,
}

impl Client {
    /// A client against the real API.
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
        // Checked here so `events_url` can extend path segments infallibly.
        snafu::ensure!(
            !parsed.cannot_be_a_base(),
            NotABaseUrlSnafu { input: base_url }
        );
        Ok(Self {
            http: http_client()?,
            auth,
            base_url: parsed,
        })
    }

    /// List events on `calendar_id` matching `query`, oldest first.
    ///
    /// Recurring events are expanded into their instances (`singleEvents`),
    /// which is also what permits ordering by start time. Pagination follows
    /// `nextPageToken` until `query.max_events` is reached.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn list_events(&self, calendar_id: &str, query: &EventQuery) -> Result<Vec<Event>> {
        let token = self.auth.access_token().await?;
        let mut events: Vec<Event> = Vec::new();
        let mut page_token: Option<String> = None;

        while events.len() < query.max_events {
            let remaining = query.max_events - events.len();
            let mut url = self.events_url(calendar_id, None);
            {
                let mut pairs = url.query_pairs_mut();
                pairs
                    .append_pair("singleEvents", "true")
                    .append_pair("orderBy", "startTime")
                    .append_pair("maxResults", &remaining.min(MAX_PAGE_SIZE).to_string());
                if let Some(min) = &query.time_min {
                    pairs.append_pair("timeMin", &min.to_rfc3339());
                }
                if let Some(max) = &query.time_max {
                    pairs.append_pair("timeMax", &max.to_rfc3339());
                }
                if let Some(text) = &query.text {
                    pairs.append_pair("q", text);
                }
                if let Some(next) = &page_token {
                    pairs.append_pair("pageToken", next);
                }
            }

            let response = self
                .http
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .context(HttpSnafu)?;
            let page: EventsPage = decode(response).await?;
            events.extend(page.items);

            match page.next_page_token {
                Some(next) if events.len() < query.max_events => page_token = Some(next),
                _ => break,
            }
        }

        events.truncate(query.max_events);
        Ok(events)
    }

    /// Fetch one event by id.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors (404 for an unknown id).
    pub async fn get_event(&self, calendar_id: &str, event_id: &str) -> Result<Event> {
        let token = self.auth.access_token().await?;
        let url = self.events_url(calendar_id, Some(event_id));
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Create an event and return it as the API stored it.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn create_event(
        &self,
        calendar_id: &str,
        draft: &EventDraft,
        send_updates: SendUpdates,
    ) -> Result<Event> {
        let token = self.auth.access_token().await?;
        let mut url = self.events_url(calendar_id, None);
        url.query_pairs_mut()
            .append_pair("sendUpdates", send_updates.as_param());
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(draft)
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Cancel (delete) an event.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors; cancelling an already-cancelled
    /// event surfaces the API's 410.
    pub async fn cancel_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        send_updates: SendUpdates,
    ) -> Result<()> {
        let token = self.auth.access_token().await?;
        let mut url = self.events_url(calendar_id, Some(event_id));
        url.query_pairs_mut()
            .append_pair("sendUpdates", send_updates.as_param());
        let response = self
            .http
            .delete(url)
            .bearer_auth(token)
            .send()
            .await
            .context(HttpSnafu)?;
        check_status(response).await?;
        Ok(())
    }

    /// URL for the events collection of `calendar_id`, or one event in it.
    /// Calendar ids are emails or `primary`; path-segment encoding handles
    /// the `@`.
    fn events_url(&self, calendar_id: &str, event_id: Option<&str>) -> Url {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .expect("with_base_url rejects cannot-be-a-base URLs");
            segments.extend(["calendars", calendar_id, "events"]);
            segments.extend(event_id);
        }
        url
    }
}

/// One HTTP client, built the same way everywhere in the crate (the API
/// client and the OAuth token client).
pub(crate) fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder().build().context(BuildClientSnafu)
}

/// Map a non-success status onto [`Error::Api`] with the message from
/// Google's error envelope; pass a success through. The one owner of API
/// error mapping, shared by body-decoding calls and bodyless ones (delete).
async fn check_status(response: reqwest::Response) -> Result<reqwest::Response> {
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
async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    check_status(response)
        .await?
        .json()
        .await
        .context(HttpSnafu)
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
        let body = r#"{"error":{"code":404,"message":"Not Found","errors":[]}}"#;
        assert_eq!(api_message(body), "Not Found");
    }

    #[test]
    fn api_message_falls_back_to_the_raw_body() {
        assert_eq!(api_message(" upstream exploded "), "upstream exploded");
    }
}
