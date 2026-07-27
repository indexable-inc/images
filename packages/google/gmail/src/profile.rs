//! `users.getProfile`: the cheapest call that proves the whole chain works.
//!
//! Reading the token file proves only that a file exists. This endpoint
//! costs one quota unit and exercises the parts that actually break --
//! refresh-token validity, scope coverage, network reachability -- and
//! returns the address the grant belongs to, which is the one fact an
//! operator needs to notice they authorized the wrong account.

use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

use crate::error::HttpSnafu;
use crate::{Client, Result, decode};

/// The authenticated mailbox, as returned by `users.getProfile`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// Address of the mailbox the grant belongs to.
    pub email_address: String,
    /// Total messages in the mailbox.
    #[serde(default)]
    pub messages_total: u64,
    /// Total threads in the mailbox.
    #[serde(default)]
    pub threads_total: u64,
    /// Mailbox history id at the time of the call.
    #[serde(default)]
    pub history_id: String,
}

impl Client {
    /// Fetch the authenticated mailbox's profile.
    ///
    /// Use this as a liveness probe: it is the smallest request that fails
    /// for every reason a caller cares about (revoked grant, missing scope,
    /// no network) rather than succeeding against a stale local file.
    ///
    /// # Errors
    /// Returns auth, transport, or API errors.
    pub async fn get_profile(&self) -> Result<Profile> {
        let url = self.user_url(["profile"]);
        let response = self.get(url).await?.send().await.context(HttpSnafu)?;
        decode(response).await
    }
}
