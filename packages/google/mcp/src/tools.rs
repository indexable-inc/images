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
use rmcp::model::{ErrorCode, ErrorData, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use google_calendar::{
    AttendeeDraft, EventDraft, EventQuery, PRIMARY_CALENDAR, SendUpdates, parse_event_end,
    parse_event_time,
};
use google_gmail::{Attachment, MessageFormat, MessageQuery, OutgoingMessage};

const SERVER: mcp_server_info::ToolServer = mcp_server_info::ToolServer {
    name: "ix-google-mcp",
    instructions: "Gmail and Google Calendar over one OAuth grant. Setup is two steps on the \
                   host: supply an OAuth client (either GOOGLE_OAUTH_CLIENT_ID and \
                   GOOGLE_OAUTH_CLIENT_SECRET, or your own client_secret.json downloaded from \
                   the Google Cloud Console), then run `gmail auth` once to consent. \
                   `google_status` reports exactly which of those is missing.",
};

/// The MCP server.
///
/// Holds the API clients when the host is set up, and the reason it is not
/// when it is not. Both are serviceable states: an unconfigured server still
/// answers `google_status` and still tells the agent what to do about it.
#[derive(Clone)]
pub struct GoogleMcp {
    clients: Option<crate::Clients>,
    /// Why [`crate::build_clients`] failed, kept verbatim for the operator.
    setup_error: Option<Arc<str>>,
    /// SMTP submission, when the host configured it. Independent of the
    /// Google clients: sending works with neither an OAuth client nor a
    /// Google account.
    smtp: Option<Arc<mail_smtp::SmtpSender>>,
    tool_router: ToolRouter<Self>,
}

impl GoogleMcp {
    /// Build the server. Never fails.
    ///
    /// A missing OAuth client or an empty token store is something to report,
    /// not something to die of: an MCP client that watches its server exit
    /// during startup shows the user nothing actionable, and takes the
    /// working half of the tool surface down with the broken half.
    #[must_use]
    pub fn new() -> Self {
        let (clients, setup_error) = match crate::build_clients() {
            Ok(clients) => (Some(clients), None),
            Err(error) => (None, Some(Arc::from(format!("{error:#}").as_str()))),
        };
        // A half-configured SMTP block is a mistake worth surfacing, not a
        // reason to refuse to start; it lands in the status report instead.
        let smtp = match mail_smtp::SmtpSender::from_env() {
            Ok(sender) => sender.map(Arc::new),
            Err(error) => {
                tracing::warn!("SMTP is configured but unusable: {error}");
                None
            }
        };
        Self {
            clients,
            setup_error,
            smtp,
            tool_router: Self::tool_router(),
        }
    }

    /// The Gmail client, or an error naming the next step.
    fn gmail(&self) -> Result<&Arc<google_gmail::Client>, ErrorData> {
        self.clients
            .as_ref()
            .map(|clients| &clients.gmail)
            .ok_or_else(|| self.not_set_up())
    }

    /// The Calendar client, or an error naming the next step.
    fn calendar(&self) -> Result<&Arc<google_calendar::Client>, ErrorData> {
        self.clients
            .as_ref()
            .map(|clients| &clients.calendar)
            .ok_or_else(|| self.not_set_up())
    }

    /// The error every tool returns while the host is unconfigured. It
    /// carries the real cause and the tool that explains the fix, so the
    /// agent can resolve this with the user instead of reporting a failure.
    fn not_set_up(&self) -> ErrorData {
        let cause = self
            .setup_error
            .as_deref()
            .unwrap_or("no Google OAuth client is configured");
        let smtp_hint = if self.smtp.is_some() {
            "\n\nSMTP is configured, so `mail_send_message` works; only the Google-backed \
             tools (reading, labels, calendar) need this."
        } else {
            ""
        };
        ErrorData::new(
            ErrorCode::INVALID_REQUEST,
            format!(
                "{cause}\n\nCall `google_status` for the full setup state and next \
                 step.{smtp_hint}"
            ),
            None,
        )
    }

    /// Verify every capability and report what is broken, once the handshake
    /// is complete.
    ///
    /// `instructions` already told the agent the locally-knowable state.
    /// This is the second audience: stderr is where an MCP host collects a
    /// stdio server's output and shows it to the operator, and it is what
    /// SEP-2577 points to now that protocol logging is deprecated.
    ///
    /// Runs after `initialize`, never during it. Probing inside the handshake
    /// would let one unreachable Google endpoint stall the connection, which
    /// is a worse failure than the misconfiguration being reported.
    pub(crate) async fn announce_health(self) {
        let health = match &self.clients {
            Some(clients) => crate::health::Health::probe(clients).await,
            None => crate::health::Health::offline(),
        }
        .with_smtp(self.smtp.as_deref());
        if !health.any_broken() {
            tracing::info!("Google integration ready\n{}", health.report());
            return;
        }
        let report = health.report();
        if health.all_broken() {
            tracing::error!("Google integration is not working:\n{report}");
        } else {
            tracing::warn!("Google integration is partly working:\n{report}");
        }
    }

    async fn delete_draft(&self, id: &str) -> Result<String, ErrorData> {
        acknowledged(self.gmail()?.delete_draft(id).await, "deleted", id)
    }

    async fn list_messages(&self, query: MessageQuery) -> Result<String, ErrorData> {
        let messages = self
            .gmail()?
            .list_messages(&query)
            .await
            .map_err(into_tool_error)?;
        json_string(&messages)
    }

    async fn mutate_message(
        &self,
        id: &str,
        mutation: MessageMutation,
    ) -> Result<String, ErrorData> {
        match mutation {
            MessageMutation::Trash => {
                acknowledged(self.gmail()?.trash_message(id).await, "trashed", id)
            }
            MessageMutation::Untrash => {
                acknowledged(self.gmail()?.untrash_message(id).await, "untrashed", id)
            }
        }
    }
}

enum MessageMutation {
    Trash,
    Untrash,
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
            .calendar()?
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
            .calendar()?
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
        let start = parse_event_time(&args.start, args.all_day, "start").map_err(invalid_params)?;
        let end = parse_event_end(&args.end, args.all_day).map_err(invalid_params)?;
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
            .calendar()?
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
        self.calendar()?
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
        self.list_messages(args.into_query()).await
    }

    #[tool(description = "List Gmail messages by filter (no free-text query). \
                       Returns ids and thread ids.")]
    async fn mail_list_messages(
        &self,
        Parameters(args): Parameters<MailListMessagesArgs>,
    ) -> Result<String, ErrorData> {
        self.list_messages(args.into_query()).await
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
            .gmail()?
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
        let threads = self
            .gmail()?
            .list_threads(&args.into_query())
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
            .gmail()?
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

        // SMTP wins when it is configured, because configuring it is a
        // deliberate act: nobody sets IX_SMTP_HOST by accident, and the
        // people who do are the ones with no Google project to fall back on.
        if let Some(smtp) = self.smtp.as_ref() {
            let raw = google_gmail::build_rfc5322(&message).map_err(into_tool_error)?;
            // The envelope, not the headers, decides delivery -- so it must
            // carry Bcc recipients, who never appear in the headers.
            let recipients: Vec<String> = message
                .to
                .iter()
                .chain(&message.cc)
                .chain(&message.bcc)
                .cloned()
                .collect();
            smtp.send(&recipients, &raw)
                .await
                .map_err(|error| ErrorData::new(ErrorCode::INTERNAL_ERROR, error.to_string(), None))?;
            return Ok(json!({
                "sent": true,
                "via": "smtp",
                "from": smtp.from_address(),
                "recipients": recipients,
            })
            .to_string());
        }

        let sent = self
            .gmail()?
            .send_message(&message)
            .await
            .map_err(into_tool_error)?;
        json_string(&sent)
    }

    // -----------------------------------------------------------------
    // Readiness
    // -----------------------------------------------------------------

    #[tool(
        description = "Check what this server can actually do right now: whether an OAuth \
                       client is configured, whether a grant is stored, and -- by making one \
                       cheap live call per capability -- whether mail and calendar really \
                       work. Call this before concluding a failure is the user's fault, and \
                       whenever a tool reports the server is not set up. Reports each \
                       capability separately; a working mailbox does not imply a working \
                       calendar."
    )]
    async fn google_status(
        &self,
        Parameters(_args): Parameters<EmptyArgs>,
    ) -> Result<String, ErrorData> {
        // Probe when we can, fall back to the local view when there is no
        // client to probe with. Never an error: "it is broken, here is why"
        // is the successful outcome of asking.
        let health = match &self.clients {
            Some(clients) => crate::health::Health::probe(clients).await,
            None => crate::health::Health::offline(),
        }
        .with_smtp(self.smtp.as_deref());
        json_string(&health)
    }

    #[tool(description = "Save a Gmail draft from the same fields as mail_send_message.")]
    async fn mail_draft_create(
        &self,
        Parameters(args): Parameters<MailComposeArgs>,
    ) -> Result<String, ErrorData> {
        let message = build_outgoing(args)?;
        let draft = self
            .gmail()?
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
            .gmail()?
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
        json_tool_result(self.gmail()?.get_draft(&args.draft_id).await)
    }

    #[tool(description = "List Gmail drafts.")]
    async fn mail_draft_list(
        &self,
        Parameters(args): Parameters<MailDraftListArgs>,
    ) -> Result<String, ErrorData> {
        json_tool_result(self.gmail()?.list_drafts(args.max_results.unwrap_or(20)).await)
    }

    #[tool(description = "Delete a Gmail draft by id.")]
    async fn mail_draft_delete(
        &self,
        Parameters(args): Parameters<MailDraftIdArgs>,
    ) -> Result<String, ErrorData> {
        self.delete_draft(&args.draft_id).await
    }

    #[tool(description = "Send a previously saved Gmail draft by id.")]
    async fn mail_draft_send(
        &self,
        Parameters(args): Parameters<MailDraftIdArgs>,
    ) -> Result<String, ErrorData> {
        json_tool_result(self.gmail()?.send_draft(&args.draft_id).await)
    }

    // -----------------------------------------------------------------
    // Gmail: mutations on a single message
    // -----------------------------------------------------------------

    #[tool(description = "Archive a Gmail message (remove the INBOX label).")]
    async fn mail_archive(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        json_tool_result(self.gmail()?.archive_message(&args.message_id).await)
    }

    #[tool(description = "Move a Gmail message to Trash.")]
    async fn mail_trash(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        self.mutate_message(&args.message_id, MessageMutation::Trash)
            .await
    }

    #[tool(description = "Restore a Gmail message from Trash.")]
    async fn mail_untrash(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        self.mutate_message(&args.message_id, MessageMutation::Untrash)
            .await
    }

    #[tool(description = "Mark a Gmail message read (remove UNREAD).")]
    async fn mail_mark_read(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        json_tool_result(self.gmail()?.mark_message_read(&args.message_id).await)
    }

    #[tool(description = "Mark a Gmail message unread (add UNREAD).")]
    async fn mail_mark_unread(
        &self,
        Parameters(args): Parameters<MailMessageIdArgs>,
    ) -> Result<String, ErrorData> {
        json_tool_result(self.gmail()?.mark_message_unread(&args.message_id).await)
    }

    // -----------------------------------------------------------------
    // Gmail: labels
    // -----------------------------------------------------------------

    #[tool(description = "List Gmail labels (system + user).")]
    async fn mail_label_list(
        &self,
        Parameters(_): Parameters<EmptyArgs>,
    ) -> Result<String, ErrorData> {
        let labels = self.gmail()?.list_labels().await.map_err(into_tool_error)?;
        json_string(&labels)
    }

    #[tool(description = "Apply a Gmail label to a message.")]
    async fn mail_label_apply(
        &self,
        Parameters(args): Parameters<MailLabelMutateArgs>,
    ) -> Result<String, ErrorData> {
        let message = self
            .gmail()?
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
            .gmail()?
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
            .gmail()?
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
        // Answered synchronously during `initialize`, so this reads the
        // environment and the token file and nothing else. The live probe
        // runs after the handshake and reports through `notifications/message`
        // and `google_status`.
        let instructions = format!(
            "{}{}",
            SERVER.instructions,
            crate::health::Health::offline()
                .with_smtp(self.smtp.as_deref())
                .instructions_block()
        );
        SERVER.live_info(env!("CARGO_PKG_VERSION"), &instructions)
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

impl MailSearchArgs {
    fn into_query(self) -> MessageQuery {
        message_query(
            Some(self.query),
            self.label_ids,
            self.include_spam_trash,
            self.max_results,
        )
    }
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

impl MailListMessagesArgs {
    fn into_query(self) -> MessageQuery {
        message_query(
            self.q,
            self.label_ids,
            self.include_spam_trash,
            self.max_results,
        )
    }
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

fn message_query(
    q: Option<String>,
    label_ids: Option<Vec<String>>,
    include_spam_trash: Option<bool>,
    max_results: Option<usize>,
) -> MessageQuery {
    MessageQuery {
        q,
        label_ids: label_ids.unwrap_or_default(),
        include_spam_trash: include_spam_trash.unwrap_or(false),
        max_results: max_results.unwrap_or(20),
    }
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, ErrorData> {
    serde_json::to_string(value).map_err(|err| {
        ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            format!("serializing response: {err}"),
            None,
        )
    })
}

fn json_tool_result<T, E>(result: Result<T, E>) -> Result<String, ErrorData>
where
    T: serde::Serialize,
    E: std::fmt::Display,
{
    let value = result.map_err(into_tool_error)?;
    json_string(&value)
}

fn acknowledged<E>(result: Result<(), E>, action: &str, id: &str) -> Result<String, ErrorData>
where
    E: std::fmt::Display,
{
    result.map_err(into_tool_error)?;
    let payload = serde_json::Map::from_iter([(action.to_owned(), json!(id))]);
    Ok(serde_json::Value::Object(payload).to_string())
}

fn into_tool_error<E: std::fmt::Display>(err: E) -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, err.to_string(), None)
}

/// Map a bad-input failure onto an `INVALID_PARAMS` tool error (the client
/// sent something malformed), distinct from [`into_tool_error`]'s
/// `INTERNAL_ERROR` for failures on our side.
fn invalid_params<E: std::fmt::Display>(err: E) -> ErrorData {
    ErrorData::new(ErrorCode::INVALID_PARAMS, err.to_string(), None)
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
    use google_calendar::SendUpdates;
    use google_gmail::MessageFormat;
    use rmcp::model::ErrorCode;

    use super::{message_format, send_updates};

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
