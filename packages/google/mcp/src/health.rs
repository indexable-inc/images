//! Per-capability health: what this server can actually do right now.
//!
//! The distinction this module exists to make is between *configured* and
//! *working*. A client id in the environment and a token file on disk say
//! nothing about whether the grant was revoked yesterday, covers the scopes
//! the tools need, or can reach Google from this host. So each capability
//! is checked by making its cheapest real call, and the results are
//! reported separately -- a working mailbox must not hide a broken
//! calendar behind one green boolean.
//!
//! Two consumers, deliberately different:
//!
//! - [`Health::offline`] answers instantly from the environment and the
//!   token file. `initialize` is answered with this, because blocking the
//!   MCP handshake on network I/O turns a fixable misconfiguration into a
//!   server that never finishes connecting.
//! - [`Health::probe`] makes the live calls, under a timeout, after the
//!   handshake. Its result reaches the operator through a logging
//!   notification and the `google_status` tool.

use std::fmt::Write as _;
use std::time::Duration;

use google_auth::scopes::{CALENDAR_EVENTS, GMAIL_MODIFY, GMAIL_SEND};
use google_auth::{ClientSecrets, TokenStore};
use serde::Serialize;

use crate::Clients;

/// How long a single liveness probe may take before it is reported as a
/// timeout. Short enough that probing every capability stays well inside a
/// human's patience, long enough that a cold TLS handshake on a slow link
/// is not mistaken for a broken grant.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What one capability can do right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// A live call succeeded.
    Ok,
    /// Prerequisites are missing; the operator has not finished setup.
    NotConfigured,
    /// Prerequisites look present but the live call failed.
    Failing,
    /// Not probed yet. Only ever seen before the first probe completes.
    Unknown,
}

impl State {
    /// A fixed-width marker, so a column of these lines up in a log.
    const fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok  ",
            Self::NotConfigured => "todo",
            Self::Failing => "fail",
            Self::Unknown => "?   ",
        }
    }
}

/// One capability's verdict, with the detail an operator needs to fix it.
#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    /// Stable identifier, e.g. `mail.send`.
    pub name: &'static str,
    /// What this capability can do right now.
    pub state: State,
    /// Human-readable evidence: the account reached, or why it failed.
    pub detail: String,
}

impl Capability {
    fn new(name: &'static str, state: State, detail: impl Into<String>) -> Self {
        Self {
            name,
            state,
            detail: detail.into(),
        }
    }
}

/// The whole server's readiness.
#[derive(Debug, Clone, Serialize)]
pub struct Health {
    /// Whether an OAuth client identity was found, and where.
    pub client_source: String,
    /// Whether a grant is stored, and where it lives.
    pub token_path: Option<String>,
    /// Per-capability verdicts.
    pub capabilities: Vec<Capability>,
    /// True once [`Self::probe`] has run; false for an offline snapshot.
    pub verified: bool,
}

impl Health {
    /// Fold a configured SMTP endpoint into the report.
    ///
    /// SMTP satisfies `mail.send` on its own -- no OAuth client, no Google
    /// account -- so a host with SMTP set up must not be told sending is
    /// broken just because Google is not configured.
    #[must_use]
    pub fn with_smtp(mut self, smtp: Option<&mail_smtp::SmtpSender>) -> Self {
        if let Some(smtp) = smtp {
            self.apply(Capability::new(
                "mail.send",
                State::Ok,
                format!("SMTP: {}", smtp.describe()),
            ));
        }
        self
    }

    /// The instantly-knowable state: environment variables, a client-secrets
    /// file, the token file and the scopes it records. No network, so this
    /// is safe to call while answering `initialize`.
    #[must_use]
    pub fn offline() -> Self {
        let (client_source, configured) = match ClientSecrets::load() {
            Ok(_) => ("found".to_owned(), true),
            Err(error) => (error.to_string(), false),
        };
        let store = TokenStore::new().ok();
        let token_path = store
            .as_ref()
            .map(|store| store.path().display().to_string());
        let stored = store.as_ref().and_then(|store| store.load().ok());

        let capabilities = [
            ("mail.read", GMAIL_MODIFY),
            ("mail.send", GMAIL_SEND),
            ("calendar", CALENDAR_EVENTS),
        ]
        .into_iter()
        .map(|(name, scope)| {
            let Some(token) = stored.as_ref() else {
                return Capability::new(
                    name,
                    State::NotConfigured,
                    if configured {
                        "OAuth client found, but nobody has consented yet; run `gmail auth`"
                    } else {
                        "no OAuth client; call `google_status` for the setup steps"
                    },
                );
            };
            if token.scopes.iter().any(|granted| granted == scope) {
                Capability::new(name, State::Unknown, "granted; not yet verified live")
            } else {
                Capability::new(
                    name,
                    State::NotConfigured,
                    format!("stored grant is missing scope `{scope}`; re-run consent"),
                )
            }
        })
        .collect();

        Self {
            client_source,
            token_path,
            capabilities,
            verified: false,
        }
    }

    /// Verify each capability by making its cheapest live call.
    ///
    /// Never returns an error: a failed probe *is* the result. Each call is
    /// bounded by [`PROBE_TIMEOUT`] so one unreachable endpoint cannot hang
    /// the report for the others.
    pub async fn probe(clients: &Clients) -> Self {
        let mut health = Self::offline();
        health.verified = true;

        // One Gmail call answers both mail capabilities: it proves the grant
        // refreshes and the mailbox is reachable. Send is then a scope
        // question, already settled by the offline pass.
        let mail = match timed(clients.gmail.get_profile()).await {
            Ok(Ok(profile)) => Capability::new(
                "mail.read",
                State::Ok,
                format!(
                    "{} ({} messages)",
                    profile.email_address, profile.messages_total
                ),
            ),
            Ok(Err(error)) => Capability::new("mail.read", State::Failing, error.to_string()),
            Err(()) => Capability::new("mail.read", State::Failing, "timed out reaching Gmail"),
        };

        // Cheapest calendar read: one event from the primary calendar.
        let calendar = match timed(clients.calendar.list_events(
            google_calendar::PRIMARY_CALENDAR,
            &google_calendar::EventQuery {
                time_min: None,
                time_max: None,
                text: None,
                max_events: 1,
            },
        ))
        .await
        {
            Ok(Ok(_)) => Capability::new("calendar", State::Ok, "primary calendar readable"),
            Ok(Err(error)) => Capability::new("calendar", State::Failing, error.to_string()),
            Err(()) => Capability::new("calendar", State::Failing, "timed out reaching Calendar"),
        };

        health.apply(mail);
        health.apply(calendar);
        health.settle_send();
        health
    }

    /// Overwrite one capability's verdict, keeping the declared order.
    fn apply(&mut self, verdict: Capability) {
        if let Some(slot) = self
            .capabilities
            .iter_mut()
            .find(|existing| existing.name == verdict.name)
        {
            *slot = verdict;
        }
    }

    /// `mail.send` has no read-only probe -- exercising it would send mail --
    /// so it inherits the mailbox's reachability and keeps its own scope
    /// verdict. Stated explicitly rather than reported as verified, because
    /// claiming a check that never ran is how a green dashboard lies.
    fn settle_send(&mut self) {
        let mail_state = self
            .capabilities
            .iter()
            .find(|capability| capability.name == "mail.read")
            .map(|capability| capability.state);
        let Some(send) = self
            .capabilities
            .iter_mut()
            .find(|capability| capability.name == "mail.send")
        else {
            return;
        };
        // Only ever upgrades an `Unknown`, so an SMTP verdict (already `Ok`)
        // and a missing-scope verdict both survive.
        match (mail_state, send.state) {
            (Some(State::Ok), State::Unknown) => {
                send.state = State::Ok;
                "scope granted, mailbox reachable".clone_into(&mut send.detail);
            }
            (Some(State::Failing), State::Unknown) => {
                send.state = State::Failing;
                "mailbox unreachable; see mail.read".clone_into(&mut send.detail);
            }
            _ => {}
        }
    }

    /// True when nothing works: worth shouting about at connect.
    #[must_use]
    pub fn all_broken(&self) -> bool {
        self.capabilities
            .iter()
            .all(|capability| capability.state != State::Ok)
    }

    /// Whether any capability is not currently usable.
    #[must_use]
    pub fn any_broken(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability.state != State::Ok)
    }

    /// A compact report, one capability per line.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = String::new();
        for capability in &self.capabilities {
            let _ = writeln!(
                out,
                "  {marker} {name:<10} {detail}",
                marker = capability.state.marker(),
                name = capability.name,
                detail = capability.detail,
            );
        }
        if let Some(path) = &self.token_path {
            let _ = writeln!(out, "  grant: {path}");
        }
        out
    }

    /// The block appended to the server's MCP `instructions`, so the agent
    /// learns the real state at connect and can raise it with the user
    /// without being asked.
    #[must_use]
    pub fn instructions_block(&self) -> String {
        if !self.any_broken() {
            return String::new();
        }
        format!(
            "\n\nSETUP STATE (as of connect{}):\n{}\n\
             Tell the user about any line above that is not `ok` -- they cannot see this. \
             Call `google_status` for the live check and the exact next step.\n",
            if self.verified {
                ", verified live"
            } else {
                ", from local config only"
            },
            self.report()
        )
    }
}

/// Run one probe under [`PROBE_TIMEOUT`], mapping elapsed time to `Err(())`.
async fn timed<F: Future>(future: F) -> Result<F::Output, ()> {
    tokio::time::timeout(PROBE_TIMEOUT, future)
        .await
        .map_err(|_elapsed| ())
}

#[cfg(test)]
mod tests {
    use super::{Capability, Health, State};

    fn health(states: [(&'static str, State); 3]) -> Health {
        Health {
            client_source: "found".to_owned(),
            token_path: None,
            capabilities: states
                .into_iter()
                .map(|(name, state)| Capability::new(name, state, "detail"))
                .collect(),
            verified: true,
        }
    }

    #[test]
    fn a_broken_calendar_is_not_hidden_by_working_mail() {
        let health = health([
            ("mail.read", State::Ok),
            ("mail.send", State::Ok),
            ("calendar", State::Failing),
        ]);

        assert!(health.any_broken(), "one failing capability must show");
        assert!(!health.all_broken(), "mail still works");
        assert!(
            health.instructions_block().contains("calendar"),
            "the agent must be told which one broke: {}",
            health.instructions_block()
        );
    }

    #[test]
    fn a_fully_working_server_adds_nothing_to_instructions() {
        let health = health([
            ("mail.read", State::Ok),
            ("mail.send", State::Ok),
            ("calendar", State::Ok),
        ]);

        assert!(health.instructions_block().is_empty());
    }

    #[test]
    fn send_inherits_mailbox_reachability_only_when_its_scope_was_granted() {
        let mut health = health([
            ("mail.read", State::Ok),
            ("mail.send", State::Unknown),
            ("calendar", State::Ok),
        ]);
        health.settle_send();
        assert_eq!(health.capabilities[1].state, State::Ok);

        // A missing scope is a verdict of its own and must survive a
        // reachable mailbox, or the report would promise a send that 403s.
        let mut missing = health;
        missing.capabilities[1].state = State::NotConfigured;
        missing.settle_send();
        assert_eq!(missing.capabilities[1].state, State::NotConfigured);
    }

    #[test]
    fn an_unverified_report_says_so() {
        let mut health = health([
            ("mail.read", State::Unknown),
            ("mail.send", State::Unknown),
            ("calendar", State::Unknown),
        ]);
        health.verified = false;

        assert!(
            health.instructions_block().contains("local config only"),
            "an unprobed report must not read as verified"
        );
    }
}
