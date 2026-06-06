//! `users.messages.*`: list, get, modify labels, trash/untrash, plus the
//! well-known system label ids used by archive/read/unread helpers.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::model::{Message, MessageQuery};
use crate::{Client, Result, decode, send_no_body};

/// Gmail's `INBOX` system label. Removing it archives the message.
pub const LABEL_INBOX: &str = "INBOX";

/// Gmail's `UNREAD` system label. Removing it marks the message read.
pub const LABEL_UNREAD: &str = "UNREAD";

/// Which projection of a message to fetch. The wire `format` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageFormat {
    /// `id`, `threadId`, `labelIds`, `snippet`, `historyId`,
    /// `internalDate`, `sizeEstimate`. No headers or body.
    Minimal,
    /// `Minimal` plus the parsed payload tree (headers, MIME parts, inline
    /// body bytes). The default, and what most reads want.
    Full,
    /// `Minimal` plus the original RFC 5322 source as base64url in
    /// [`Message::raw`]. Use when you want to forward the message verbatim.
    Raw,
    /// `Minimal` plus the payload-level headers; bodies are not returned.
    Metadata,
}

impl MessageFormat {
    pub(crate) const fn as_param(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Full => "full",
            Self::Raw => "raw",
            Self::Metadata => "metadata",
        }
    }
}

impl Default for MessageFormat {
    fn default() -> Self {
        Self::Full
    }
}

/// One page of `users.messages.list`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessagesPage {
    #[serde(default)]
    messages: Vec<MessageStub>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// `messages.list` returns only ids and thread ids on the page; the caller
/// fetches each one's payload through `get_message`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageStub {
    /// Opaque message id.
    pub id: String,
    /// Thread the message belongs to.
    pub thread_id: String,
}

/// The body of `users.messages.modify`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModifyRequest<'a> {
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    add_label_ids: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    remove_label_ids: &'a [String],
}

impl Client {
    /// List message ids matching `query`. Most recent first. Pagination
    /// follows `nextPageToken` until `query.max_results` is reached.
    ///
    /// The page returns only ids and thread ids; call [`Self::get_message`]
    /// on each id to read headers and bodies.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn list_messages(&self, query: &MessageQuery) -> Result<Vec<MessageStub>> {
        let mut out: Vec<MessageStub> = Vec::new();
        let mut page_token: Option<String> = None;

        while out.len() < query.max_results {
            let remaining = query.max_results - out.len();
            let mut url = self.user_url(["messages"]);
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair(
                    "maxResults",
                    &remaining.min(crate::MAX_PAGE_SIZE).to_string(),
                );
                if query.include_spam_trash {
                    pairs.append_pair("includeSpamTrash", "true");
                }
                if let Some(q) = &query.q {
                    pairs.append_pair("q", q);
                }
                for label in &query.label_ids {
                    pairs.append_pair("labelIds", label);
                }
                if let Some(next) = &page_token {
                    pairs.append_pair("pageToken", next);
                }
            }

            let response = self.get(url).await?.send().await.context(HttpSnafu)?;
            let page: MessagesPage = decode(response).await?;
            out.extend(page.messages);

            match page.next_page_token {
                Some(next) if out.len() < query.max_results => page_token = Some(next),
                _ => break,
            }
        }

        out.truncate(query.max_results);
        Ok(out)
    }

    /// Fetch one message by id at the chosen projection.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors (404 for an unknown id).
    pub async fn get_message(&self, id: &str, format: MessageFormat) -> Result<Message> {
        let mut url = self.user_url(["messages", id]);
        url.query_pairs_mut()
            .append_pair("format", format.as_param());
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        decode(response).await
    }

    /// Add and remove labels on a message in one call. Returns the
    /// post-modify projection.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn modify_labels(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<Message> {
        let url = self.user_url(["messages", id, "modify"]);
        let response = self
            .post(url)
            .await?
            .json(&ModifyRequest {
                add_label_ids: add,
                remove_label_ids: remove,
            })
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Move a message to Trash.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn trash_message(&self, id: &str) -> Result<()> {
        let url = self.user_url(["messages", id, "trash"]);
        send_no_body(self.post(url).await?).await
    }

    /// Restore a message from Trash.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn untrash_message(&self, id: &str) -> Result<()> {
        let url = self.user_url(["messages", id, "untrash"]);
        send_no_body(self.post(url).await?).await
    }

    /// Remove the `INBOX` label. Mirrors the Gmail UI's "Archive".
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn archive_message(&self, id: &str) -> Result<Message> {
        self.modify_labels(id, &[], &[LABEL_INBOX.to_owned()]).await
    }

    /// Remove the `UNREAD` label.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn mark_message_read(&self, id: &str) -> Result<Message> {
        self.modify_labels(id, &[], &[LABEL_UNREAD.to_owned()]).await
    }

    /// Add the `UNREAD` label.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn mark_message_unread(&self, id: &str) -> Result<Message> {
        self.modify_labels(id, &[LABEL_UNREAD.to_owned()], &[]).await
    }
}
