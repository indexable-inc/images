//! `users.labels.*`: list and get. Label add/remove on messages lives on
//! [`Client::modify_labels`] in [`crate::messages`].

use serde::Deserialize;
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::model::Label;
use crate::{Client, Result, decode};

#[derive(Deserialize)]
struct LabelsList {
    #[serde(default)]
    labels: Vec<Label>,
}

impl Client {
    /// List every label on the mailbox (system + user).
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        let url = self.user_url(["labels"]);
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        let envelope: LabelsList = decode(response).await?;
        Ok(envelope.labels)
    }

    /// Fetch one label by id (including its message/unread totals).
    ///
    /// # Errors
    /// Returns auth, transport, or API errors (404 for an unknown id).
    pub async fn get_label(&self, id: &str) -> Result<Label> {
        let url = self.user_url(["labels", id]);
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        decode(response).await
    }
}
