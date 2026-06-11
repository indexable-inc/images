//! `ix-google-mcp`: a stdio MCP server that exposes Gmail and Google
//! Calendar to an MCP client (Claude, Codex) in one process, sharing one
//! [`google_auth`] grant.
//!
//! The server is intentionally a thin binding per RFC 0003: every tool
//! handler does argument shaping, calls a method on the underlying
//! `google_gmail::Client` or `google_calendar::Client`, and returns the
//! crate's wire JSON. Domain logic and OAuth refresh live in the core
//! crates, not here.
//!
//! Auth bootstrap is out of band: `gmail auth` (or `gcal auth`) on the
//! workstation mints the refresh token and writes it to the shared store.
//! This server then refreshes access tokens on demand. The
//! [`google_auth::Authenticator`] cache is expiry-aware, so one
//! `Authenticator` per process is enough for the server's lifetime.

mod tools;

use std::sync::Arc;

use anyhow::Context as _;
use google_auth::scopes::{CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND};
use google_auth::{Authenticator, ClientSecrets, TokenStore};
use rmcp::ServiceExt as _;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let server = tools::GoogleMcp::new().context("building the ix-google-mcp server")?;
    let service = server
        .serve(stdio())
        .await
        .context("starting the MCP service over stdio")?;
    service.waiting().await?;
    Ok(())
}

fn init_logging() {
    // Log to stderr only; stdout is reserved for the MCP wire protocol.
    let filter =
        EnvFilter::try_from_env("IX_GOOGLE_MCP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

/// The API clients backing the tools.
///
/// They share the on-disk token store (one refresh token) but hold their
/// own expiry-aware access-token caches, so a refresh in one does not
/// block the other. If Google rotates the refresh token during one
/// client's refresh, the other client's in-flight refresh can lose that
/// race and fail once; its next mint re-reads the rotated token the
/// winner persisted, so the failure heals without re-consent.
pub(crate) struct Clients {
    pub(crate) calendar: Arc<google_calendar::Client>,
    pub(crate) gmail: Arc<google_gmail::Client>,
}

/// Construct the API clients from the environment and the token store.
pub(crate) fn build_clients() -> anyhow::Result<Clients> {
    let secrets = ClientSecrets::from_env().context(
        "ix-google-mcp expects GOOGLE_OAUTH_CLIENT_ID and GOOGLE_OAUTH_CLIENT_SECRET in the \
         environment; run gmail auth (or gcal auth) on the host to mint the refresh token",
    )?;
    let store = TokenStore::new()?;
    let scopes: &[&str] = &[CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND];

    let calendar_auth =
        Authenticator::new(secrets.clone(), store.clone(), scopes).context("calendar auth")?;
    let gmail_auth = Authenticator::new(secrets, store, scopes).context("gmail auth")?;

    let calendar =
        google_calendar::Client::new(calendar_auth).context("building the calendar client")?;
    let gmail = google_gmail::Client::new(gmail_auth).context("building the gmail client")?;
    Ok(Clients {
        calendar: Arc::new(calendar),
        gmail: Arc::new(gmail),
    })
}
