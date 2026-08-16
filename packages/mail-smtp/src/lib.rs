//! SMTP submission: send mail as anyone, without a Google Cloud project.
//!
//! The Gmail API path needs an OAuth client, a consent flow, and a stored
//! refresh token. That is the right shape for reading a mailbox, and the
//! wrong shape for the common case of *sending* -- it excludes everyone on
//! Fastmail, Proton Bridge, iCloud, Zoho, Exchange or a self-hosted server,
//! none of whom have a Google project to point at.
//!
//! This module takes an already-built RFC 5322 message and submits it. It
//! deliberately does not build messages: the MIME builder in
//! `google_gmail::build_rfc5322` is shared, so a header-handling fix cannot
//! reach one transport and miss the other.
//!
//! # Gmail via SMTP
//!
//! A personal Gmail account works here with a 16-character app password
//! (Account → Security → 2-Step Verification → App passwords) and no Cloud
//! project at all. A Google **Workspace** account does not: Google ended
//! password authentication for Workspace on 2025-05-01, so those accounts
//! must use the Gmail API path. Accounts on Advanced Protection cannot
//! create app passwords either.

use std::time::Duration;

use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};
use snafu::{OptionExt as _, ResultExt as _, Snafu, ensure};

/// Environment variable naming the SMTP host. Its presence is what marks
/// the SMTP backend as configured.
pub const HOST_ENV: &str = "IX_SMTP_HOST";
/// Environment variable naming the submission port. Defaults to 587.
pub const PORT_ENV: &str = "IX_SMTP_PORT";
/// Environment variable naming the SMTP username (usually the full address).
pub const USER_ENV: &str = "IX_SMTP_USER";
/// Environment variable naming the SMTP password or app password.
pub const PASSWORD_ENV: &str = "IX_SMTP_PASSWORD";
/// Environment variable naming the envelope sender. Defaults to the user.
pub const FROM_ENV: &str = "IX_SMTP_FROM";

/// Submission port using STARTTLS, the modern default.
const DEFAULT_PORT: u16 = 587;
/// Submission port using implicit TLS.
const IMPLICIT_TLS_PORT: u16 = 465;

/// How long a submission may take. Generous enough for a slow relay,
/// bounded so an agent waiting on a send cannot hang indefinitely.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Failures from SMTP submission.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// A required setting is missing.
    #[snafu(display(
        "SMTP is not fully configured: {name} is unset. Needed: {HOST_ENV}, {USER_ENV}, \
         {PASSWORD_ENV} (an app password for a personal Gmail account)"
    ))]
    Missing {
        /// The variable that was missing.
        name: &'static str,
    },

    /// The port was set to something that is not a port number.
    #[snafu(display("{PORT_ENV} is not a port number: {value:?}"))]
    BadPort {
        /// What was supplied.
        value: String,
    },

    /// The envelope could not be built from the supplied addresses.
    #[snafu(display("invalid envelope address {address:?}: {detail}"))]
    BadAddress {
        /// The address that failed to parse.
        address: String,
        /// Why it failed.
        detail: String,
    },

    /// No recipient was supplied, so there is nobody to submit to.
    #[snafu(display("an outgoing message needs at least one recipient"))]
    NoRecipients,

    /// The transport could not be constructed.
    #[snafu(display("could not build the SMTP transport for {host}: {source}"))]
    Transport {
        /// Host that was being connected to.
        host: String,
        /// Underlying lettre error.
        source: lettre::transport::smtp::Error,
    },

    /// The server rejected the message or the connection failed.
    #[snafu(display("SMTP submission to {host} failed: {source}"))]
    Send {
        /// Host that rejected it.
        host: String,
        /// Underlying lettre error.
        source: lettre::transport::smtp::Error,
    },
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A configured SMTP submission endpoint.
///
/// No `Debug`: the password is a live credential and must not ride into
/// logs or panic messages, the same rule `google_auth::ClientSecrets` follows.
#[derive(Clone)]
pub struct SmtpSender {
    host: String,
    port: u16,
    username: String,
    password: String,
    from: String,
}

impl SmtpSender {
    /// Read the endpoint from the environment.
    ///
    /// Returns `Ok(None)` when [`HOST_ENV`] is unset -- SMTP simply is not
    /// the configured backend, which is not an error. A host with an
    /// incomplete configuration *is* an error, because it means somebody
    /// tried and got it half right, and silently falling back would hide
    /// that.
    ///
    /// # Errors
    /// Returns [`Error::Missing`] when the host is set but a credential is
    /// not, and [`Error::BadPort`] for an unparseable port.
    pub fn from_env() -> Result<Option<Self>> {
        let Some(host) = non_empty(HOST_ENV) else {
            return Ok(None);
        };
        let username = non_empty(USER_ENV).context(MissingSnafu { name: USER_ENV })?;
        let password = non_empty(PASSWORD_ENV).context(MissingSnafu { name: PASSWORD_ENV })?;
        let port = match non_empty(PORT_ENV) {
            None => DEFAULT_PORT,
            Some(value) => value.parse().ok().context(BadPortSnafu {
                value: value.clone(),
            })?,
        };
        let from = non_empty(FROM_ENV).unwrap_or_else(|| username.clone());
        Ok(Some(Self {
            host,
            port,
            username,
            password,
            from,
        }))
    }

    /// The address this endpoint sends as.
    #[must_use]
    pub fn from_address(&self) -> &str {
        &self.from
    }

    /// A description safe to put in a status report: host, port and user,
    /// never the password.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{user} via {host}:{port}",
            user = self.username,
            host = self.host,
            port = self.port
        )
    }

    /// Submit an already-built RFC 5322 message.
    ///
    /// `recipients` is the SMTP envelope, which is what actually decides
    /// delivery -- it must include Bcc recipients, who by definition do not
    /// appear in the headers.
    ///
    /// # Errors
    /// Returns [`Error::NoRecipients`] for an empty envelope,
    /// [`Error::BadAddress`] for an unparseable address, and transport or
    /// submission errors otherwise.
    pub async fn send(&self, recipients: &[String], rfc5322: &[u8]) -> Result<()> {
        ensure!(!recipients.is_empty(), NoRecipientsSnafu);

        let from = parse_address(&self.from)?;
        let to = recipients
            .iter()
            .map(|address| parse_address(address))
            .collect::<Result<Vec<_>>>()?;
        let envelope =
            lettre::address::Envelope::new(Some(from), to).map_err(|error| Error::BadAddress {
                address: self.from.clone(),
                detail: error.to_string(),
            })?;

        // Port 465 is implicit TLS; everything else is STARTTLS on a plain
        // connection. Both are encrypted -- there is no cleartext path here.
        let builder = if self.port == IMPLICIT_TLS_PORT {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
        };
        let transport = builder
            .context(TransportSnafu {
                host: self.host.clone(),
            })?
            .port(self.port)
            .timeout(Some(TIMEOUT))
            .credentials(Credentials::new(
                self.username.clone(),
                self.password.clone(),
            ))
            .build();

        transport
            .send_raw(&envelope, rfc5322)
            .await
            .context(SendSnafu {
                host: self.host.clone(),
            })?;
        Ok(())
    }
}

/// Parse one address for the SMTP envelope.
fn parse_address(address: &str) -> Result<lettre::Address> {
    // The envelope takes a bare addr-spec, never a display name, so strip a
    // `Name <addr>` wrapper rather than rejecting it: callers reasonably
    // pass the same strings they put in the headers.
    let bare = match (address.rfind('<'), address.rfind('>')) {
        (Some(open), Some(close)) if open < close => &address[open + 1..close],
        _ => address,
    };
    bare.trim()
        .parse()
        .map_err(|error: lettre::address::AddressError| Error::BadAddress {
            address: address.to_owned(),
            detail: error.to_string(),
        })
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{Error, SmtpSender, parse_address};

    #[test]
    fn a_bare_address_parses() {
        assert_eq!(
            parse_address("a@example.com").expect("parses").to_string(),
            "a@example.com"
        );
    }

    #[test]
    fn a_display_name_is_stripped_rather_than_rejected() {
        // Callers pass the same strings they put in To:, and the envelope
        // takes an addr-spec only.
        let parsed = parse_address("Ada Lovelace <ada@example.com>").expect("parses");
        assert_eq!(parsed.to_string(), "ada@example.com");
    }

    #[test]
    fn a_nonsense_address_names_itself_in_the_error() {
        let error = parse_address("not-an-address").expect_err("rejects");
        assert!(
            matches!(&error, Error::BadAddress { address, .. } if address == "not-an-address"),
            "got: {error:?}"
        );
    }

    #[tokio::test]
    async fn an_empty_envelope_is_refused_before_connecting() {
        let sender = SmtpSender {
            host: "smtp.invalid".to_owned(),
            port: 587,
            username: "u@example.com".to_owned(),
            password: "secret".to_owned(),
            from: "u@example.com".to_owned(),
        };

        let error = sender.send(&[], b"raw").await.expect_err("no recipients");

        assert!(matches!(error, Error::NoRecipients), "got: {error:?}");
    }

    #[test]
    fn describe_never_leaks_the_password() {
        let sender = SmtpSender {
            host: "smtp.example.com".to_owned(),
            port: 587,
            username: "u@example.com".to_owned(),
            password: "hunter2".to_owned(),
            from: "u@example.com".to_owned(),
        };

        let described = sender.describe();

        assert!(described.contains("smtp.example.com"));
        assert!(
            !described.contains("hunter2"),
            "status output must not carry the credential"
        );
    }
}
