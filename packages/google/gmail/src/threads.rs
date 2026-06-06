//! `users.threads.*`: list and get.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::messages::MessageFormat;
use crate::model::{MessageQuery, Thread};
use crate::{Client, Result, decode};

/// One page of `users.threads.list`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadsPage {
    #[serde(default)]
    threads: Vec<ThreadStub>,
    #[serde(default)]
    next_page_token: Option<String>,
}

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
        let mut out: Vec<ThreadStub> = Vec::new();
        let mut page_token: Option<String> = None;

        while out.len() < query.max_results {
            let remaining = query.max_results - out.len();
            let mut url = self.user_url(["threads"]);
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
            let page: ThreadsPage = decode(response).await?;
            out.extend(page.threads);

            match page.next_page_token {
                Some(next) if out.len() < query.max_results => page_token = Some(next),
                _ => break,
            }
        }

        out.truncate(query.max_results);
        Ok(out)
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
