//! `users.drafts.*` and `users.messages.send`: compose, save, update, list,
//! delete, and send.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::mime::build_raw;
use crate::model::{Draft, Message, OutgoingMessage};
use crate::{Client, Result, decode, send_no_body};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftRequest {
    message: MessageRaw,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageRaw {
    /// Base64url-encoded RFC 5322 source.
    raw: String,
    /// Existing thread id when replying.
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
}

/// One page of `users.drafts.list`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftsPage {
    #[serde(default)]
    drafts: Vec<DraftStub>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// `drafts.list` returns ids and the wrapped message id; the caller fetches
/// the body by calling `get_draft`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftStub {
    /// Opaque draft id.
    pub id: String,
    /// The message resource the draft wraps.
    pub message: DraftMessageStub,
}

/// The wrapped-message stub inside a [`DraftStub`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftMessageStub {
    /// Opaque message id of the draft's contents.
    pub id: String,
    /// Thread the draft will land in when sent.
    pub thread_id: String,
}

impl Client {
    /// Send a freshly composed message.
    ///
    /// # Errors
    /// Returns [`crate::Error::UnsafeHeader`] for header-injection-shaped
    /// inputs, plus auth, transport, and API errors.
    pub async fn send_message(&self, message: &OutgoingMessage) -> Result<Message> {
        let url = self.user_url(["messages", "send"]);
        let response = self
            .post(url)
            .await?
            .json(&MessageRaw {
                raw: build_raw(message)?,
                thread_id: message.thread_id.clone(),
            })
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Save a draft. The returned draft carries the id used by
    /// [`Self::send_draft`] / [`Self::update_draft`] / [`Self::delete_draft`].
    ///
    /// # Errors
    /// Returns [`crate::Error::UnsafeHeader`] for header-injection-shaped
    /// inputs, plus auth, transport, and API errors.
    pub async fn create_draft(&self, message: &OutgoingMessage) -> Result<Draft> {
        let url = self.user_url(["drafts"]);
        let response = self
            .post(url)
            .await?
            .json(&DraftRequest {
                message: MessageRaw {
                    raw: build_raw(message)?,
                    thread_id: message.thread_id.clone(),
                },
            })
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Replace a draft's contents with a fresh outgoing message.
    ///
    /// # Errors
    /// Returns [`crate::Error::UnsafeHeader`] for header-injection-shaped
    /// inputs, plus auth, transport, and API errors.
    pub async fn update_draft(&self, draft_id: &str, message: &OutgoingMessage) -> Result<Draft> {
        let url = self.user_url(["drafts", draft_id]);
        let response = self
            .put(url)
            .await?
            .json(&DraftRequest {
                message: MessageRaw {
                    raw: build_raw(message)?,
                    thread_id: message.thread_id.clone(),
                },
            })
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }

    /// Fetch one draft by id.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors (404 for an unknown id).
    pub async fn get_draft(&self, id: &str) -> Result<Draft> {
        let url = self.user_url(["drafts", id]);
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        decode(response).await
    }

    /// List drafts. `max_results` caps the result; pagination follows
    /// `nextPageToken`.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn list_drafts(&self, max_results: usize) -> Result<Vec<DraftStub>> {
        let mut out: Vec<DraftStub> = Vec::new();
        let mut page_token: Option<String> = None;

        while out.len() < max_results {
            let remaining = max_results - out.len();
            let mut url = self.user_url(["drafts"]);
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair(
                    "maxResults",
                    &remaining.min(crate::MAX_PAGE_SIZE).to_string(),
                );
                if let Some(next) = &page_token {
                    pairs.append_pair("pageToken", next);
                }
            }
            let response = self.get(url).await?.send().await.context(HttpSnafu)?;
            let page: DraftsPage = decode(response).await?;
            out.extend(page.drafts);

            match page.next_page_token {
                Some(next) if out.len() < max_results => page_token = Some(next),
                _ => break,
            }
        }

        out.truncate(max_results);
        Ok(out)
    }

    /// Delete a draft.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn delete_draft(&self, id: &str) -> Result<()> {
        let url = self.user_url(["drafts", id]);
        send_no_body(self.delete(url).await?).await
    }

    /// Send a previously saved draft.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn send_draft(&self, draft_id: &str) -> Result<Message> {
        let url = self.user_url(["drafts", "send"]);
        let response = self
            .post(url)
            .await?
            .json(&serde_json::json!({ "id": draft_id }))
            .send()
            .await
            .context(HttpSnafu)?;
        decode(response).await
    }
}
