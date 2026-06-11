//! Wire types for the Gmail v1 [`users.messages`, `users.threads`,
//! `users.labels`, `users.drafts`] resources.
//!
//! Field names mirror the upstream camelCase JSON so the same types serve
//! the HTTP client, the `gmail --json` output, and the MCP tool results:
//! the tool surface and the CLI surface cannot drift (RFC 0003). Only the
//! fields the surfaces actually use are modeled; unknown upstream fields
//! are ignored on read and never invented on write.

use chrono::{DateTime, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};

/// One Gmail message, as returned by `users.messages.get` /
/// `users.messages.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Opaque message id, the handle for `get`/`modify`/`trash`.
    pub id: String,
    /// Thread the message belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Labels currently applied to the message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_ids: Vec<String>,
    /// A short preview Gmail computes for the inbox view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Gmail's record of when the message was received, as a UTC instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_internal_date",
        serialize_with = "serialize_internal_date"
    )]
    pub internal_date: Option<DateTime<Utc>>,
    /// Parsed payload tree. Absent on the `minimal` projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<MessagePart>,
    /// Gmail's history id watermark, used by `users.history.list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
    /// Approximate size in bytes; useful for budgeting attachment downloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_estimate: Option<u64>,
    /// Base64url-encoded RFC 5322 source. Only set for the `raw` projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// One node in a parsed MIME tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePart {
    /// Identifier within the part tree; empty on the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    /// The part's MIME type, e.g. `text/plain` or `multipart/alternative`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Filename for attachment parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Headers on this part. The root carries the message-level headers
    /// (From, To, Subject, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<Header>,
    /// Inline body bytes (for leaf parts) or attachment handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<MessagePartBody>,
    /// Child parts for multipart MIME containers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Self>,
}

impl MessagePart {
    /// The value of the first header named `name` (case-insensitive), if any.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }
}

/// One MIME header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    /// Header name, e.g. `From`, `Subject`.
    pub name: String,
    /// Header value as Gmail decoded it (RFC 2047 unfolded).
    pub value: String,
}

/// A part's body: inline data, attachment handle, or both empty for a
/// container.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePartBody {
    /// Inline bytes, base64url-encoded. Absent on attachment leaves and on
    /// container parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Size in bytes of the decoded body.
    #[serde(default)]
    pub size: u64,
    /// Handle for `users.messages.attachments.get`, set on attachment leaves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
}

/// One thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// Opaque thread id.
    pub id: String,
    /// Preview text from the most recent message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    /// Gmail history watermark when the thread was last touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_id: Option<String>,
    /// Messages in the thread, oldest first; populated only by
    /// [`crate::Client::get_thread`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<Message>,
}

/// One label.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    /// Opaque label id (use this with `messages.modify`).
    pub id: String,
    /// User-visible name (`INBOX`, `Family`, `Receipts/2026`).
    pub name: String,
    /// Whether Gmail shows the label in its sidebar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_list_visibility: Option<String>,
    /// Whether Gmail shows the label on the message-list row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_list_visibility: Option<String>,
    /// `system` for built-ins (`INBOX`, `STARRED`, ...), `user` for the rest.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Messages currently carrying the label (Gmail's estimate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_total: Option<u64>,
    /// Unread subset of `messages_total`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_unread: Option<u64>,
}

/// One draft.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    /// Opaque draft id.
    pub id: String,
    /// The drafted message. Carries the same payload as a sent message.
    pub message: Message,
}

/// Selection for [`crate::Client::list_messages`] /
/// [`crate::Client::list_threads`].
#[derive(Debug, Clone, Default)]
pub struct MessageQuery {
    /// Gmail search syntax (`from:`, `to:`, `newer_than:`, `label:`, etc.).
    pub q: Option<String>,
    /// Restrict to threads carrying every label in this set.
    pub label_ids: Vec<String>,
    /// Include spam and trash in the result; off by default.
    pub include_spam_trash: bool,
    /// Upper bound on returned items; pagination follows `nextPageToken`
    /// until it is reached.
    pub max_results: usize,
}

/// A message to send or save as a draft.
#[derive(Debug, Clone, Default)]
pub struct OutgoingMessage {
    /// Primary recipients.
    pub to: Vec<String>,
    /// Carbon-copy recipients.
    pub cc: Vec<String>,
    /// Blind-carbon-copy recipients.
    pub bcc: Vec<String>,
    /// Subject line; bare newlines and control characters are rejected by
    /// [`crate::Client::send_message`] / [`crate::Client::create_draft`].
    pub subject: String,
    /// Plain-text body. At least one of [`Self::body_text`] or
    /// [`Self::body_html`] must be set.
    pub body_text: Option<String>,
    /// HTML body. Sent as `text/html` next to `body_text` in a
    /// `multipart/alternative` when both are present.
    pub body_html: Option<String>,
    /// Thread to attach the message to (a reply). The recipient list and
    /// `In-Reply-To`/`References` are still the caller's responsibility:
    /// Gmail will reject a reply that does not share the thread's subject.
    pub thread_id: Option<String>,
    /// Files to attach.
    pub attachments: Vec<Attachment>,
}

/// One outgoing attachment.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// Display name in the recipient's client.
    pub filename: String,
    /// `Content-Type` to send the attachment as.
    pub content_type: String,
    /// Raw bytes; base64-encoded into the wire by the send path.
    pub content: Vec<u8>,
}

/// Gmail's `internalDate` is the receive-time in milliseconds since the
/// epoch, encoded as a JSON string. Round-trip through a typed `DateTime`
/// so the surfaces above this crate never re-implement the conversion.
fn deserialize_internal_date<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    let raw: Option<String> = Option::deserialize(deserializer)?;
    let Some(raw) = raw else { return Ok(None) };
    let millis: i64 = raw
        .parse()
        .map_err(|err| D::Error::custom(format!("internalDate is not an integer: {err}")))?;
    Utc.timestamp_millis_opt(millis).single().map_or_else(
        || {
            Err(D::Error::custom(
                "internalDate is out of the representable range",
            ))
        },
        |dt| Ok(Some(dt)),
    )
}

// serde's `serialize_with` calls this with `&Option<T>` because the field
// is `Option<T>`; we cannot widen the param to `Option<&T>` without a
// wrapper. The match here reads the inner value by reference.
#[allow(
    clippy::ref_option,
    reason = "shape dictated by serde's serialize_with"
)]
fn serialize_internal_date<S>(
    instant: &Option<DateTime<Utc>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match instant.as_ref() {
        None => serializer.serialize_none(),
        Some(instant) => serializer.serialize_str(&instant.timestamp_millis().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::Message;

    #[test]
    fn internal_date_round_trips_through_string_millis() {
        let json = r#"{"id":"abc","internalDate":"1717575600000"}"#;
        let message: Message = serde_json::from_str(json).expect("parses");
        let instant = message.internal_date.expect("has date");
        assert_eq!(instant.timestamp_millis(), 1_717_575_600_000);

        // Round-trip back to the wire shape.
        let written = serde_json::to_value(&message).expect("serializes");
        assert_eq!(written["internalDate"], "1717575600000");
    }

    #[test]
    fn minimal_projection_parses_without_payload_or_headers() {
        let json = r#"{"id":"abc","threadId":"def","labelIds":["INBOX","UNREAD"]}"#;
        let message: Message = serde_json::from_str(json).expect("parses");
        assert_eq!(message.id, "abc");
        assert_eq!(message.thread_id.as_deref(), Some("def"));
        assert_eq!(message.label_ids, vec!["INBOX", "UNREAD"]);
        assert!(message.payload.is_none());
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let json = r#"{"headers":[{"name":"Subject","value":"Hi"}]}"#;
        let part: super::MessagePart = serde_json::from_str(json).expect("parses");
        assert_eq!(part.header("subject"), Some("Hi"));
        assert_eq!(part.header("SUBJECT"), Some("Hi"));
        assert_eq!(part.header("missing"), None);
    }
}
