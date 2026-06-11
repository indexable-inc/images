//! Tool surface exposed by `ix-google-mcp`. Each tool is a thin shaper
//! over a [`google_calendar::Client`] or [`google_gmail::Client`] method.
//!
//! Tool naming: calendar tools keep the `calendar_*` prefix already used
//! by the Python `FastMCP` they replace; mail tools use `mail_*` and
//! match `superhuman-mail`'s surface 1:1 first (RFC 0003 + #599), so
//! swapping `superhuman-mail` out for this server is a single config
//! change for every agent already wired to it.

use std::sync::Arc;

use chrono::{DateTime, FixedOffset};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ErrorCode, ErrorData, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use google_calendar::{
    AttendeeDraft, EventDraft, EventQuery, EventTime, PRIMARY_CALENDAR, SendUpdates,
};
use google_gmail::{Attachment, MessageFormat, MessageQuery, OutgoingMessage};

/// The MCP server. Holds the two API clients shared across tool calls.
#[derive(Clone)]
pub struct GoogleMcp {
    calendar: Arc<google_calendar::Client>,
    gmail: Arc<google_gmail::Client>,
    tool_router: ToolRouter<Self>,
}

impl GoogleMcp {
    /// Build the server from the environment (`GOOGLE_OAUTH_CLIENT_ID`,
    /// `GOOGLE_OAUTH_CLIENT_SECRET`) and the on-disk token store.
    ///
    /// # Errors
    /// Returns an error if the env vars are unset, the store is empty, or
    /// either API client cannot be constructed.
    pub fn new() -> anyhow::Result<Self> {
        let crate::Clients { calendar, gmail } = crate::build_clients()?;
        Ok(Self {
            calendar,
            gmail,
            tool_router: Self::tool_router(),
        })
    }
}

#[tool_router(router = tool_router)]
impl GoogleMcp {
    // -----------------------------------------------------------------
    // Calendar
    // -----------------------------------------------------------------

    #[tool(
        description = "List Google Calendar events on a calendar in a window. \
                       Defaults to the user's primary calendar and the next 7 days."
    )]
    async fn calendar_events(
        &self,
        Parameters(args): Parameters<CalendarEventsArgs>,
    ) -> Result<String, ErrorData> {
        let query = EventQuery {
            time_min: args.time_min,
            time_max: args.time_max,
            text: args.text,
            max_events: args.max_events.unwrap_or(50),
        };
        let calendar = args
            .calendar_id
            .as_deref()
            .unwrap_or(PRIMARY_CALENDAR)
            .to_owned();
        let events = self
            .calendar
            .list_events(&calendar, &query)
            .await
            .map_err(into_tool_error)?;
        json_string(&events)
    }

    #[tool(description = "Get one Google Calendar event by id.")]
    async fn calendar_event_get(
        &self,
        Parameters(args): Parameters<CalendarEventGetArgs>,
    ) -> Result<String, ErrorData> {
        let calendar = args
            .calendar_id
            .as_deref()
            .unwrap_or(PRIMARY_CALENDAR)
            .to_owned();
        let event = self
            .calendar
            .get_event(&calendar, &args.event_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&event)
    }

    #[tool(
        description = "Create a Google Calendar event. start/end are RFC 3339 \
                       (with offset) for timed events, or YYYY-MM-DD for all-day \
                       events (end being the inclusive last day). notify selects \
                       who Google emails about the invite (all|external-only|none, \
                       default all)."
    )]
    async fn calendar_event_create(
        &self,
        Parameters(args): Parameters<CalendarEventCreateArgs>,
    ) -> Result<String, ErrorData> {
        let start = parse_event_time(&args.start, args.all_day, "start")?;
        let end = parse_event_end(&args.end, args.all_day)?;
        let draft = EventDraft {
            summary: args.summary,
            description: args.description,
            location: args.location,
            start,
            end,
            attendees: args
                .attendees
                .into_iter()
                .map(|email| AttendeeDraft { email })
                .collect(),
        };
        let calendar = args
            .calendar_id
            .as_deref()
            .unwrap_or(PRIMARY_CALENDAR)
            .to_owned();
        let created = self
            .calendar
            .create_event(&calendar, &draft, send_updates(args.notify.as_deref())?)
            .await
            .map_err(into_tool_error)?;
        json_string(&created)
    }

    #[tool(description = "Cancel a Google Calendar event by id.")]
    async fn calendar_event_cancel(
        &self,
        Parameters(args): Parameters<CalendarEventCancelArgs>,
    ) -> Result<String, ErrorData> {
        let calendar = args
            .calendar_id
            .as_deref()
            .unwrap_or(PRIMARY_CALENDAR)
            .to_owned();
        self.calendar
            .cancel_event(
                &calendar,
                &args.event_id,
                send_updates(args.notify.as_deref())?,
            )
            .await
            .map_err(into_tool_error)?;
        Ok(json!({ "cancelled": args.event_id }).to_string())
    }

    // -----------------------------------------------------------------
    // Gmail: search / read
    // -----------------------------------------------------------------

    #[tool(description = "Search Gmail messages with the Gmail query syntax \
                       (e.g. `from:alice newer_than:7d`). Returns ids and \
                       thread ids; use mail_get_message for headers and body.")]
    async fn mail_search(
        &self,
        Parameters(args): Parameters<MailSearchArgs>,
    ) -> Result<String, ErrorData> {
        let query = MessageQuery {
            q: Some(args.query),
            label_ids: args.label_ids.unwrap_or_default(),
            include_spam_trash: args.include_spam_trash.unwrap_or(false),
            max_results: args.max_results.unwrap_or(20),
        };
        let stubs = self
            .gmail
            .list_messages(&query)
            .await
            .map_err(into_tool_error)?;
        json_string(&stubs)
    }

    #[tool(description = "List Gmail messages by filter (no free-text query). \
                       Returns ids and thread ids.")]
    async fn mail_list_messages(
        &self,
        Parameters(args): Parameters<MailListMessagesArgs>,
    ) -> Result<String, ErrorData> {
        let query = MessageQuery {
            q: args.q,
            label_ids: args.label_ids.unwrap_or_default(),
            include_spam_trash: args.include_spam_trash.unwrap_or(false),
            max_results: args.max_results.unwrap_or(20),
        };
        let stubs = self
            .gmail
            .list_messages(&query)
            .await
            .map_err(into_tool_error)?;
        json_string(&stubs)
    }

    #[tool(description = "Fetch one Gmail message by id. format=full (default) \
                       returns headers + body; minimal returns just ids and \
                       labels; metadata returns headers without body; raw \
                       returns the RFC 5322 source as base64url.")]
    async fn mail_get_message(
        &self,
        Parameters(args): Parameters<MailGetMessageArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .get_message(&args.message_id, message_format(args.format.as_deref())?)
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    #[tool(description = "List Gmail threads matching `q` (Gmail search syntax).")]
    async fn mail_list_threads(
        &self,
        Parameters(args): Parameters<MailSearchArgs>,
    ) -> Result<String, ErrorData> {
        let query = MessageQuery {
            q: Some(args.query),
            label_ids: args.label_ids.unwrap_or_default(),
            include_spam_trash: args.include_spam_trash.unwrap_or(false),
            max_results: args.max_results.unwrap_or(20),
        };
        let threads = self
            .gmail
            .list_threads(&query)
            .await
            .map_err(into_tool_error)?;
        json_string(&threads)
    }

    #[tool(description = "Fetch one Gmail thread (with its messages) by id.")]
    async fn mail_get_thread(
        &self,
        Parameters(args): Parameters<MailGetThreadArgs>,
    ) -> Result<String, ErrorData> {
        let thread = self
            .gmail
            .get_thread(&args.thread_id, message_format(args.format.as_deref())?)
            .await
            .map_err(into_tool_error)?;
        json_string(&thread)
    }

    // -----------------------------------------------------------------
    // Gmail: send and drafts
    // -----------------------------------------------------------------

    #[tool(description = "Compose and send a Gmail message. body_text and \
                       body_html are alternatives; provide at least one. \
                       attachments are inline (base64-encoded bytes) plus \
                       filename and content_type. thread_id attaches a reply \
                       to an existing thread.")]
    async fn mail_send_message(
        &self,
        Parameters(args): Parameters<MailComposeArgs>,
    ) -> Result<String, ErrorData> {
        let message = build_outgoing(args)?;
        let sent = self
            .gmail
            .send_message(&message)
            .await
            .map_err(into_tool_error)?;
        json_string(&sent)
    }

    #[tool(description = "Save a Gmail draft from the same fields as mail_send_message.")]
    async fn mail_draft_create(
        &self,
        Parameters(args): Parameters<MailComposeArgs>,
    ) -> Result<String, ErrorData> {
        let message = build_outgoing(args)?;
        let draft = self
            .gmail
            .create_draft(&message)
            .await
            .map_err(into_tool_error)?;
        json_string(&draft)
    }

    #[tool(description = "Replace a Gmail draft's contents with a fresh composition.")]
    async fn mail_draft_update(
        &self,
        Parameters(args): Parameters<MailDraftUpdateArgs>,
    ) -> Result<String, ErrorData> {
        let message = build_outgoing(args.compose)?;
        let draft = self
            .gmail
            .update_draft(&args.draft_id, &message)
            .await
            .map_err(into_tool_error)?;
        json_string(&draft)
    }

    #[tool(description = "Fetch one Gmail draft by id.")]
    async fn mail_draft_get(
        &self,
        Parameters(args): Parameters<MailDraftIdArgs>,
    ) -> Result<String, ErrorData> {
        let draft = self
            .gmail
            .get_draft(&args.draft_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&draft)
    }

    #[tool(description = "List Gmail drafts.")]
    async fn mail_draft_list(
        &self,
        Parameters(args): Parameters<MailDraftListArgs>,
    ) -> Result<String, ErrorData> {
        let drafts = self
            .gmail
            .list_drafts(args.max_results.unwrap_or(20))
            .await
            .map_err(into_tool_error)?;
        json_string(&drafts)
    }

    #[tool(description = "Delete a Gmail draft by id.")]
    async fn mail_draft_delete(
        &self,
        Parameters(args): Parameters<MailDraftIdArgs>,
    ) -> Result<String, ErrorData> {
        self.gmail
            .delete_draft(&args.draft_id)
            .await
            .map_err(into_tool_error)?;
        Ok(json!({ "deleted": args.draft_id }).to_string())
    }

    #[tool(description = "Send a previously saved Gmail draft by id.")]
    async fn mail_draft_send(
        &self,
        Parameters(args): Parameters<MailDraftIdArgs>,
    ) -> Result<String, ErrorData> {
        let sent = self
            .gmail
            .send_draft(&args.draft_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&sent)
    }

    // -----------------------------------------------------------------
    // Gmail: mutations on a single message
    // -----------------------------------------------------------------

    #[tool(description = "Archive a Gmail message (remove the INBOX label).")]
    async fn mail_archive(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .archive_message(&args.message_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    #[tool(description = "Move a Gmail message to Trash.")]
    async fn mail_trash(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        self.gmail
            .trash_message(&args.message_id)
            .await
            .map_err(into_tool_error)?;
        Ok(json!({ "trashed": args.message_id }).to_string())
    }

    #[tool(description = "Restore a Gmail message from Trash.")]
    async fn mail_untrash(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        self.gmail
            .untrash_message(&args.message_id)
            .await
            .map_err(into_tool_error)?;
        Ok(json!({ "untrashed": args.message_id }).to_string())
    }

    #[tool(description = "Mark a Gmail message read (remove UNREAD).")]
    async fn mail_mark_read(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .mark_message_read(&args.message_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    #[tool(description = "Mark a Gmail message unread (add UNREAD).")]
    async fn mail_mark_unread(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .mark_message_unread(&args.message_id)
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    // -----------------------------------------------------------------
    // Gmail: labels
    // -----------------------------------------------------------------

    #[tool(description = "List Gmail labels (system + user).")]
    async fn mail_label_list(
        &self,
        Parameters(_): Parameters<EmptyArgs>,
    ) -> Result<String, ErrorData> {
        let labels = self.gmail.list_labels().await.map_err(into_tool_error)?;
        json_string(&labels)
    }

    #[tool(description = "Apply a Gmail label to a message.")]
    async fn mail_label_apply(
        &self,
        Parameters(args): Parameters<MailLabelMutateArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .modify_labels(&args.message_id, &[args.label_id], &[])
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    #[tool(description = "Remove a Gmail label from a message.")]
    async fn mail_label_remove(
        &self,
        Parameters(args): Parameters<MailLabelMutateArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail
            .modify_labels(&args.message_id, &[], &[args.label_id])
            .await
            .map_err(into_tool_error)?;
        json_string(&message)
    }

    // -----------------------------------------------------------------
    // Gmail: attachments
    // -----------------------------------------------------------------

    #[tool(description = "Fetch a Gmail attachment's bytes. Returns the bytes \
                       as base64 (standard padding) in the `content_base64` \
                       field plus a `size` field; the agent decodes as needed.")]
    async fn mail_attachment_get(
        &self,
        Parameters(args): Parameters<MailAttachmentGetArgs>,
    ) -> Result<String, ErrorData> {
        use base64::Engine as _;
        let bytes = self
            .gmail
            .get_attachment(&args.message_id, &args.attachment_id)
            .await
            .map_err(into_tool_error)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(json!({
            "content_base64": encoded,
            "size": bytes.len(),
        })
        .to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GoogleMcp {
    fn get_info(&self) -> ServerInfo {
        // Both ServerInfo and Implementation are #[non_exhaustive], so
        // start from a Default and patch the fields we care about.
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        "ix-google-mcp".clone_into(&mut info.server_info.name);
        env!("CARGO_PKG_VERSION").clone_into(&mut info.server_info.version);
        info.instructions = Some(
            "Gmail and Google Calendar via the shared google-auth grant. \
             Run `gmail auth` (or `gcal auth`) on the host once to mint \
             the refresh token before invoking any tool."
                .to_owned(),
        );
        info
    }
}

// ---------------------------------------------------------------------
// Argument types (schemas derived via schemars)
// ---------------------------------------------------------------------

#[derive(Deserialize, JsonSchema, Default)]
pub struct EmptyArgs {}

#[derive(Deserialize, JsonSchema)]
pub struct CalendarEventsArgs {
    /// Calendar id: an email address, or `primary` (the default).
    #[serde(default)]
    pub calendar_id: Option<String>,
    /// Inclusive lower bound on event end time, RFC 3339.
    #[serde(default)]
    pub time_min: Option<DateTime<FixedOffset>>,
    /// Exclusive upper bound on event start time, RFC 3339.
    #[serde(default)]
    pub time_max: Option<DateTime<FixedOffset>>,
    /// Free-text filter (forwarded as the `q` parameter).
    #[serde(default)]
    pub text: Option<String>,
    /// Maximum number of events. Defaults to 50.
    #[serde(default)]
    pub max_events: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CalendarEventGetArgs {
    pub event_id: String,
    #[serde(default)]
    pub calendar_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CalendarEventCreateArgs {
    pub summary: String,
    /// RFC 3339 instant (timed event) or YYYY-MM-DD (all-day).
    pub start: String,
    /// RFC 3339 instant (timed event) or YYYY-MM-DD inclusive last day
    /// (all-day; the tool converts to the API's exclusive end date).
    pub end: String,
    #[serde(default)]
    pub all_day: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub attendees: Vec<String>,
    /// Who Google emails about the invite: `all` (default), `external-only`,
    /// or `none`.
    #[serde(default)]
    pub notify: Option<String>,
    #[serde(default)]
    pub calendar_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CalendarEventCancelArgs {
    pub event_id: String,
    #[serde(default)]
    pub calendar_id: Option<String>,
    #[serde(default)]
    pub notify: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailSearchArgs {
    /// Gmail search syntax (e.g. `from:alice newer_than:7d label:work`).
    pub query: String,
    /// Restrict to messages carrying every label in this set.
    #[serde(default)]
    pub label_ids: Option<Vec<String>>,
    /// Include spam and trash in the result.
    #[serde(default)]
    pub include_spam_trash: Option<bool>,
    /// Maximum number of results. Defaults to 20.
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailListMessagesArgs {
    /// Optional Gmail search query. If omitted, the call returns the most
    /// recent messages on the mailbox, restricted by `label_ids` if set.
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub label_ids: Option<Vec<String>>,
    #[serde(default)]
    pub include_spam_trash: Option<bool>,
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailGetMessageArgs {
    pub message_id: String,
    /// `full` (default) | `minimal` | `metadata` | `raw`.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailGetThreadArgs {
    pub thread_id: String,
    /// `full` (default) | `minimal` | `metadata` | `raw`.
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailComposeArgs {
    /// Primary recipients.
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    pub subject: String,
    /// Plain-text body. At least one of `body_text` and `body_html` must
    /// be set.
    #[serde(default)]
    pub body_text: Option<String>,
    /// HTML body. Sent alongside `body_text` in a `multipart/alternative`
    /// when both are present.
    #[serde(default)]
    pub body_html: Option<String>,
    /// Thread to attach the message to (reply).
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Attachments to include with the message.
    #[serde(default)]
    pub attachments: Vec<AttachmentInput>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AttachmentInput {
    /// Display filename in the recipient's client.
    pub filename: String,
    /// MIME type (e.g. `application/pdf`).
    pub content_type: String,
    /// Standard-base64-encoded attachment bytes.
    pub content_base64: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailDraftUpdateArgs {
    pub draft_id: String,
    #[serde(flatten)]
    pub compose: MailComposeArgs,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailDraftIdArgs {
    pub draft_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailDraftListArgs {
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailMessageIdArgs {
    pub message_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailLabelMutateArgs {
    pub message_id: String,
    pub label_id: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct MailAttachmentGetArgs {
    pub message_id: String,
    pub attachment_id: String,
}

// ---------------------------------------------------------------------
// Helpers shared across tools
// ---------------------------------------------------------------------

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, ErrorData> {
    serde_json::to_string(value).map_err(|err| {
        ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            format!("serializing response: {err}"),
            None,
        )
    })
}

fn into_tool_error<E: std::fmt::Display>(err: E) -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, err.to_string(), None)
}

fn parse_event_time(
    input: &str,
    all_day: bool,
    field: &'static str,
) -> Result<EventTime, ErrorData> {
    if all_day {
        let date = input.parse().map_err(|err| {
            ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                format!("{field}: could not parse {input:?} as YYYY-MM-DD: {err}"),
                None,
            )
        })?;
        Ok(EventTime::AllDay { date })
    } else {
        let date_time = DateTime::parse_from_rfc3339(input).map_err(|err| {
            ErrorData::new(
                ErrorCode::INVALID_PARAMS,
                format!("{field}: could not parse {input:?} as RFC 3339: {err}"),
                None,
            )
        })?;
        Ok(EventTime::Timed {
            date_time,
            time_zone: None,
        })
    }
}

/// Parse the `end` of an event. All-day input is the inclusive last day
/// (how the tools document it); Google's all-day `end.date` is exclusive,
/// so convert at this boundary.
fn parse_event_end(input: &str, all_day: bool) -> Result<EventTime, ErrorData> {
    match parse_event_time(input, all_day, "end")? {
        EventTime::AllDay { date } => {
            EventTime::all_day_end_from_inclusive(date).ok_or_else(|| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!("end: no day follows {date}"),
                    None,
                )
            })
        }
        timed @ EventTime::Timed { .. } => Ok(timed),
    }
}

/// `notify` defaults to `All` only when absent; an unrecognized value is
/// an error, because it decides who Google emails.
fn send_updates(notify: Option<&str>) -> Result<SendUpdates, ErrorData> {
    notify.map_or(Ok(SendUpdates::All), |value| {
        value.parse().map_err(|err| {
            ErrorData::new(ErrorCode::INVALID_PARAMS, format!("notify: {err}"), None)
        })
    })
}

/// `format` defaults to `Full` only when absent; an unrecognized value is
/// an error rather than silently fetching full bodies.
fn message_format(format: Option<&str>) -> Result<MessageFormat, ErrorData> {
    format.map_or(Ok(MessageFormat::Full), |value| {
        value.parse().map_err(|err| {
            ErrorData::new(ErrorCode::INVALID_PARAMS, format!("format: {err}"), None)
        })
    })
}

fn build_outgoing(args: MailComposeArgs) -> Result<OutgoingMessage, ErrorData> {
    use base64::Engine as _;
    if args.body_text.is_none() && args.body_html.is_none() {
        return Err(ErrorData::new(
            ErrorCode::INVALID_PARAMS,
            "at least one of body_text or body_html is required",
            None,
        ));
    }
    let mut attachments = Vec::with_capacity(args.attachments.len());
    for attachment in args.attachments {
        let content = base64::engine::general_purpose::STANDARD
            .decode(attachment.content_base64.as_bytes())
            .map_err(|err| {
                ErrorData::new(
                    ErrorCode::INVALID_PARAMS,
                    format!(
                        "attachment {:?}: base64 decode failed: {err}",
                        attachment.filename
                    ),
                    None,
                )
            })?;
        attachments.push(Attachment {
            filename: attachment.filename,
            content_type: attachment.content_type,
            content,
        });
    }
    Ok(OutgoingMessage {
        to: args.to,
        cc: args.cc,
        bcc: args.bcc,
        subject: args.subject,
        body_text: args.body_text,
        body_html: args.body_html,
        thread_id: args.thread_id,
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use google_calendar::{EventTime, SendUpdates};
    use google_gmail::MessageFormat;
    use rmcp::model::ErrorCode;

    use super::{message_format, parse_event_end, send_updates};

    #[test]
    fn all_day_end_is_inclusive_at_the_tool_and_exclusive_on_the_wire() {
        let end = parse_event_end("2026-06-12", true).expect("parses");
        assert_eq!(
            end,
            EventTime::AllDay {
                date: "2026-06-13".parse().expect("date"),
            }
        );
    }

    #[test]
    fn absent_notify_and_format_keep_their_documented_defaults() {
        assert_eq!(send_updates(None).expect("default"), SendUpdates::All);
        assert_eq!(message_format(None).expect("default"), MessageFormat::Full);
    }

    #[test]
    fn unknown_notify_is_invalid_params_not_email_everyone() {
        let err = send_updates(Some("non")).expect_err("rejects");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("notify"), "got: {}", err.message);
    }

    #[test]
    fn unknown_format_is_invalid_params_not_full_bodies() {
        let err = message_format(Some("ful")).expect_err("rejects");
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("format"), "got: {}", err.message);
    }
}
