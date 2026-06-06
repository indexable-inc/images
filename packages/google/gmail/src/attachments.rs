//! `users.messages.attachments.get`: fetch an attachment's bytes.
//!
//! Gmail returns the body as URL-safe base64 (no padding); this module
//! decodes it so callers get the raw bytes back, ready to write to disk.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use serde::Deserialize;
use snafu::ResultExt as _;

use crate::error::{Base64Snafu, HttpSnafu};
use crate::{Client, Result, decode};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentBody {
    /// Base64url-encoded attachment bytes.
    data: String,
    /// Size of the decoded body in bytes; the wire repeats it so the
    /// caller can budget before decoding.
    #[serde(default)]
    #[allow(dead_code)] // surfaced by Gmail but not re-emitted by this client
    size: u64,
}

impl Client {
    /// Fetch and decode one attachment by id.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors; [`crate::Error::Base64`] if
    /// Gmail returned malformed bytes (a Google bug).
    pub async fn get_attachment(&self, message_id: &str, attachment_id: &str) -> Result<Bytes> {
        let url = self.user_url(["messages", message_id, "attachments", attachment_id]);
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        let body: AttachmentBody = decode(response).await?;
        let bytes = URL_SAFE_NO_PAD
            .decode(body.data.as_bytes())
            .context(Base64Snafu {
                field: "attachments.data",
            })?;
        Ok(Bytes::from(bytes))
    }
}
