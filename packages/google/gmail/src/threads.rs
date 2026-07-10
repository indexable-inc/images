//! `users.threads.*`: list and get.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::messages::MessageFormat;
use crate::model::{MessageQuery, Thread};
use crate::{Client, Result, decode};

/// `threads.list` returns only thread ids and snippets on the page; the
/// caller fetches messages by calling `get_thread`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStub {
    /// Opaque thread id.
    pub id: String,
    /// Preview text from the most recent message in the thread.
    #[serde(default)]
    pub snippet: Option<String>,
    /// History watermark when the thread was last touched.
    #[serde(default)]
    pub history_id: Option<String>,
}

impl Client {
    /// List thread ids matching `query`. Most recent first.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn list_threads(&self, query: &MessageQuery) -> Result<Vec<ThreadStub>> {
        self.list_message_resources::<ThreadStub>("threads", query)
            .await
    }

    /// Fetch one thread (with messages) by id.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors (404 for an unknown id).
    pub async fn get_thread(&self, id: &str, format: MessageFormat) -> Result<Thread> {
        let mut url = self.user_url(["threads", id]);
        url.query_pairs_mut()
            .append_pair("format", format.as_param());
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        decode(response).await
    }
}
