//! Elixir binding for Gmail via unibind: send, search, and read the
//! signed-in user's mail from the BEAM. The thin seam the Elixir kernel
//! (ix-mcp-ex) loads at runtime so a cell can say
//! `Gmail.send(to, subject, body)` (#3803); all domain logic stays in the
//! `google-gmail` crate, and OAuth stays in `google-auth` -- the same
//! grant (`~/.config/google/token.json`, minted by `gmail auth`) that the
//! CLI, the MCP tools, and the Python binding share.
//!
//! The refresh token and the client secret never cross the boundary:
//! functions return message data or `status()` metadata, exactly the
//! property the old `gcal` shell-out had.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`GmailEx`) and the OTP app (`:gmail_ex`).
#[unibind::export(backends(ex))]
mod _gmail_ex {
    use std::sync::OnceLock;

    use google_auth::scopes::{GMAIL_MODIFY, GMAIL_SEND};
    use google_auth::{Authenticator, CLIENT_ID_ENV, CLIENT_SECRET_ENV, ClientSecrets, TokenStore};
    use google_gmail::{Client, MessageFormat, MessagePart, MessageQuery, OutgoingMessage};

    /// Scopes every call here needs on the stored grant: read/modify for
    /// `search`/`show`, send for `send`.
    const REQUIRED_SCOPES: &[&str] = &[GMAIL_MODIFY, GMAIL_SEND];

    /// Boundary failures. A signed-out kernel is an `Auth` error on the
    /// mail calls but plain data on `status()`.
    #[unibind::error]
    #[derive(Debug)]
    pub enum GmailError {
        /// Sign-in problems: missing client credentials in the
        /// environment, no stored grant, a missing scope, or a revoked
        /// grant. The fix is `gmail auth` on the host.
        Auth {
            /// What the auth layer rejected.
            message: String,
        },
        /// The Gmail API answered with a non-success status.
        Api {
            /// Status and message from Google's error envelope.
            message: String,
        },
        /// Transport failure: the request never got a decodable answer.
        Http {
            /// The underlying failure.
            message: String,
        },
        /// The input cannot become a valid message or query.
        BadInput {
            /// What was wrong with the input.
            message: String,
        },
    }

    impl std::fmt::Display for GmailError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Auth { message }
                | Self::Api { message }
                | Self::Http { message }
                | Self::BadInput { message } => write!(formatter, "{message}"),
            }
        }
    }

    impl std::error::Error for GmailError {}

    /// A sent message's handles.
    #[unibind::record]
    #[derive(Clone)]
    pub struct SentMessage {
        /// The new message's id.
        pub id: String,
        /// Thread the message landed in.
        pub thread_id: Option<String>,
    }

    /// One search hit: the metadata projection of a message.
    #[unibind::record]
    #[derive(Clone)]
    pub struct MessageSummary {
        /// Message id, the handle for `show`.
        pub id: String,
        /// Thread the message belongs to.
        pub thread_id: Option<String>,
        /// Receive time as RFC 3339 UTC.
        pub date: Option<String>,
        /// The `From` header.
        pub from: Option<String>,
        /// The `Subject` header.
        pub subject: Option<String>,
        /// Gmail's inbox-view preview text.
        pub snippet: Option<String>,
    }

    /// One full message: headers, labels, and the decoded text body.
    #[unibind::record]
    #[derive(Clone)]
    pub struct MessageDetail {
        /// Message id.
        pub id: String,
        /// Thread the message belongs to.
        pub thread_id: Option<String>,
        /// Receive time as RFC 3339 UTC.
        pub date: Option<String>,
        /// The `From` header.
        pub from: Option<String>,
        /// The `To` header.
        pub to: Option<String>,
        /// The `Cc` header.
        pub cc: Option<String>,
        /// The `Subject` header.
        pub subject: Option<String>,
        /// Labels currently on the message (`INBOX`, `UNREAD`, ...).
        pub labels: Vec<String>,
        /// Gmail's preview text.
        pub snippet: Option<String>,
        /// First decoded `text/plain` part (falling back to `text/html`).
        pub body: Option<String>,
    }

    /// Whether this kernel can talk to Gmail, as data.
    #[unibind::record]
    #[derive(Clone)]
    pub struct AuthStatus {
        /// Both OAuth client env vars are present and non-empty.
        pub configured: bool,
        /// A grant is stored on disk.
        pub signed_in: bool,
        /// Scopes the stored grant covers.
        pub scopes: Vec<String>,
        /// Required scopes the stored grant is missing (empty when ready).
        pub missing_scopes: Vec<String>,
        /// Where the grant lives (`~/.config/google/token.json`).
        pub token_path: Option<String>,
    }

    /// One client (and its token cache) per BEAM node. Built on first
    /// successful use and kept for the process lifetime so the
    /// `Authenticator` can reuse minted access tokens across cells; a
    /// failed build is not cached, because the fix (exporting the client
    /// credentials, running `gmail auth`) can land while the kernel runs.
    fn client() -> Result<&'static Client, GmailError> {
        static CLIENT: OnceLock<Client> = OnceLock::new();
        if let Some(client) = CLIENT.get() {
            return Ok(client);
        }
        let auth = authenticator().map_err(|message| GmailError::Auth { message })?;
        let built = Client::new(auth).map_err(map_error)?;
        Ok(CLIENT.get_or_init(|| built))
    }

    fn authenticator() -> Result<Authenticator, String> {
        let secrets = ClientSecrets::from_env().map_err(|error| error.to_string())?;
        let store = TokenStore::new().map_err(|error| error.to_string())?;
        Authenticator::new(secrets, store, REQUIRED_SCOPES).map_err(|error| error.to_string())
    }

    fn map_error(error: google_gmail::Error) -> GmailError {
        let message = error.to_string();
        match error {
            google_gmail::Error::Auth { .. } => GmailError::Auth { message },
            google_gmail::Error::Api { .. } => GmailError::Api { message },
            google_gmail::Error::UnsafeHeader { .. }
            | google_gmail::Error::BadBaseUrl { .. }
            | google_gmail::Error::NotABaseUrl { .. } => GmailError::BadInput { message },
            _ => GmailError::Http { message },
        }
    }

    /// `input` as an address list: one address or a comma-separated list;
    /// empty and whitespace-only entries drop out.
    fn split_addresses(input: Option<String>) -> Vec<String> {
        input
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }

    fn header_of(message: &google_gmail::Message, name: &str) -> Option<String> {
        message
            .payload
            .as_ref()
            .and_then(|payload| payload.header(name))
            .map(ToOwned::to_owned)
    }

    fn summary_of(message: google_gmail::Message) -> MessageSummary {
        let from = header_of(&message, "From");
        let subject = header_of(&message, "Subject");
        MessageSummary {
            id: message.id,
            thread_id: message.thread_id,
            date: message.internal_date.map(|instant| instant.to_rfc3339()),
            from,
            subject,
            snippet: message.snippet,
        }
    }

    /// Compose and send a plain-text message; returns the new message's
    /// handles. `to`, `cc`, and `bcc` each take one address or a
    /// comma-separated list; `body_html` adds an HTML alternative next to
    /// the text; `thread_id` replies into an existing thread (the subject
    /// must still match the thread's).
    pub async fn send(
        to: String,
        subject: String,
        body_text: String,
        cc: Option<String>,
        bcc: Option<String>,
        body_html: Option<String>,
        thread_id: Option<String>,
    ) -> Result<SentMessage, GmailError> {
        let to = split_addresses(Some(to));
        if to.is_empty() {
            return Err(GmailError::BadInput {
                message: "`to` needs at least one recipient address".to_owned(),
            });
        }
        let outgoing = OutgoingMessage {
            to,
            cc: split_addresses(cc),
            bcc: split_addresses(bcc),
            subject,
            body_text: Some(body_text),
            body_html,
            thread_id,
            attachments: Vec::new(),
        };
        let sent = client()?.send_message(&outgoing).await.map_err(map_error)?;
        Ok(SentMessage {
            id: sent.id,
            thread_id: sent.thread_id,
        })
    }

    /// Search messages with the Gmail query syntax (`from:`, `to:`,
    /// `label:`, `newer_than:`, ...); most recent first. Each hit costs
    /// one metadata fetch, so `limit` is the request budget.
    pub async fn search(
        query: String,
        #[unibind(default = 10)] limit: u64,
    ) -> Result<Vec<MessageSummary>, GmailError> {
        let client = client()?;
        let query = MessageQuery {
            q: Some(query),
            max_results: usize::try_from(limit).unwrap_or(usize::MAX),
            ..MessageQuery::default()
        };
        // The list page carries only ids; hydrate each hit's headers the
        // way the CLI's listing does.
        let stubs = client.list_messages(&query).await.map_err(map_error)?;
        let mut summaries = Vec::with_capacity(stubs.len());
        for stub in stubs {
            let message = client
                .get_message(&stub.id, MessageFormat::Metadata)
                .await
                .map_err(map_error)?;
            summaries.push(summary_of(message));
        }
        Ok(summaries)
    }

    /// One message by id: headers, labels, snippet, and the decoded text
    /// body.
    pub async fn show(id: String) -> Result<MessageDetail, GmailError> {
        let message = client()?
            .get_message(&id, MessageFormat::Full)
            .await
            .map_err(map_error)?;
        Ok(MessageDetail {
            date: message.internal_date.map(|instant| instant.to_rfc3339()),
            from: header_of(&message, "From"),
            to: header_of(&message, "To"),
            cc: header_of(&message, "Cc"),
            subject: header_of(&message, "Subject"),
            body: message.payload.as_ref().and_then(MessagePart::text_body),
            id: message.id,
            thread_id: message.thread_id,
            labels: message.label_ids,
            snippet: message.snippet,
        })
    }

    /// Whether this kernel can talk to Gmail: client credentials present,
    /// a grant stored, and which required scopes it covers. Reads the
    /// environment and the token file only -- no network, no tokens in the
    /// result. Blocking (DirtyIo) for the file read.
    #[unibind(blocking)]
    pub fn status() -> AuthStatus {
        let configured = env_present(CLIENT_ID_ENV) && env_present(CLIENT_SECRET_ENV);
        let store = TokenStore::new().ok();
        let token_path = store
            .as_ref()
            .map(|store| store.path().display().to_string());
        let stored = store.as_ref().and_then(|store| store.load().ok());
        let scopes = stored
            .as_ref()
            .map(|token| token.scopes.clone())
            .unwrap_or_default();
        let missing_scopes = REQUIRED_SCOPES
            .iter()
            .filter(|required| !scopes.iter().any(|scope| scope == *required))
            .map(|scope| (*scope).to_owned())
            .collect();
        AuthStatus {
            configured,
            signed_in: stored.is_some(),
            scopes,
            missing_scopes,
            token_path,
        }
    }

    fn env_present(name: &str) -> bool {
        std::env::var(name).is_ok_and(|value| !value.is_empty())
    }
}
