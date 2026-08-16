//! Answer credential requests from a token file the host already holds.
//!
//! The lending path in [`crate::helper`] borrows a credential from an
//! operator's workstation. A VM provisioned from the ix secret store is the
//! other case: the token is already on the box, at a path the deployment
//! chose, and nothing needs to be borrowed. That would otherwise be written
//! per-consumer as a shell script rendering
//! `https://user:pass@host` into a file for `git-credential-store`.
//!
//! Rendering that URL is the part worth avoiding. It makes `@ : /` and `%`
//! structural in a value that is not a URL component, so a token containing
//! one silently produces a credential file that matches nothing. git's own
//! wire format has no such hazard: a value is the rest of the line, and
//! [`Message::push`] already refuses the only byte that could forge a field
//! boundary. So this speaks the protocol directly and never builds a URL.

use std::fs;
use std::io::{self, BufReader, Write};
use std::path::Path;

use color_eyre::eyre::{Result, bail};

use crate::protocol::Message;

/// Why a token file cannot produce a credential.
///
/// Separate variants because the operator's next move differs: store a
/// secret, fix a mode, or find out who truncated the file. git sees one
/// outcome for all three, so the distinction has to be made here.
#[derive(Debug, PartialEq, Eq)]
pub enum Unusable {
    /// No file at the path. The token was never delivered.
    Missing,
    /// The file is there but this process cannot read it, usually an owner
    /// or mode that does not match the account the consumer runs as.
    Unreadable(String),
    /// Present and readable but carries no token. A truncated write or an
    /// empty value in the store. Called out separately because it is the
    /// state that most looks like success from the outside.
    Empty,
}

impl Unusable {
    /// A diagnostic naming the path and what to do next.
    #[must_use]
    pub fn describe(&self, path: &Path) -> String {
        let path = path.display();
        match self {
            Self::Missing => format!(
                "no token at {path}: the secret was not delivered.\n\
                 Store one and re-apply:\n    ix secret set github_token"
            ),
            Self::Unreadable(detail) => format!(
                "the token at {path} cannot be read: {detail}\n\
                 Check the owner and mode against the account this runs as."
            ),
            Self::Empty => format!(
                "the token at {path} is empty.\n\
                 Re-store it; an empty value authenticates as nobody and a \
                 private repository answers 404 rather than 401."
            ),
        }
    }
}

/// Read the token, treating the two empty-ish states as failures.
///
/// Both otherwise render a credential that is structurally present and
/// carries no secret, which authenticates as an anonymous user. On a private
/// repository that surfaces as `404`, pointing at the URL rather than at the
/// credential.
///
/// # Errors
///
/// The file is absent, unreadable, or holds nothing but whitespace.
pub fn read(path: &Path) -> Result<String, Unusable> {
    match fs::read_to_string(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(Unusable::Missing),
        Err(error) => Err(Unusable::Unreadable(error.to_string())),
        Ok(raw) => {
            // Trailing newline is how every editor and `ix secret set` writes
            // a one-line file; it is not part of the token.
            let token = raw.trim();
            if token.is_empty() {
                Err(Unusable::Empty)
            } else {
                Ok(token.to_owned())
            }
        }
    }
}

/// The credential to answer a matching request with.
///
/// # Errors
///
/// The token carries a byte that cannot ride in the protocol.
fn credential(token: &str, username: &str) -> Result<Message> {
    let mut reply = Message::default();
    reply.push("username", username)?;
    reply.push("password", token)?;
    Ok(reply)
}

/// Whether this request is one the helper should answer.
///
/// git scopes a helper by config, so this is the second of two gates rather
/// than the only one. It exists because a helper is an ordinary executable:
/// anything on the box can run it and read its stdout. Re-checking the
/// request means the token cannot leave for another host even when the
/// caller is not git.
fn answers(request: &Message, hosts: &[String]) -> bool {
    // A host-less request is git asking for a default credential; there is no
    // host to check against, so the guard cannot pass it.
    let Some(host) = request.get("host") else {
        return false;
    };
    // A cleartext protocol would put the token on the wire. `protocol` is
    // absent only in the same default-credential case.
    if request.get("protocol") != Some("https") {
        return false;
    }
    hosts.iter().any(|allowed| allowed == host)
}

/// git's credential helper protocol, answered from a token file.
///
/// Exits zero whatever happens, matching [`crate::helper::run`]: a helper is
/// registered for every git operation on the host, so failing hard here turns
/// "no token yet" into "git is broken for everybody". Diagnostics go to
/// stderr, where they appear exactly when the operator was about to see an
/// authentication failure anyway. A caller that needs the absent token to be
/// fatal calls [`check`] first; that is what the git-clone unit does.
///
/// # Errors
///
/// Only a failure to read git's request or write its response.
pub fn helper(operation: &str, path: &Path, hosts: &[String], username: &str) -> Result<()> {
    let request = Message::read(&mut BufReader::new(io::stdin().lock()))?;

    // `store` and `erase` are drained and ignored: the token's lifetime is
    // the deployment's to manage, and accepting a write would imply this
    // helper owns a store it can change.
    if operation != "get" || !answers(&request, hosts) {
        return Ok(());
    }

    match read(path) {
        Ok(token) => credential(&token, username)?.write(&mut io::stdout().lock()),
        Err(unusable) => {
            report(&unusable.describe(path));
            Ok(())
        }
    }
}

/// Fail loudly if the token file cannot produce a credential.
///
/// The preflight for a unit that is about to need the token. Run before a
/// clone, it turns a `404` that reads as a wrong URL into a message naming
/// the missing secret.
///
/// # Errors
///
/// The token is absent, unreadable, or empty.
pub fn check(path: &Path) -> Result<()> {
    match read(path) {
        Ok(_) => Ok(()),
        Err(unusable) => bail!("{}", unusable.describe(path)),
    }
}

/// Say why on stderr, then let git carry on and fail its own way.
fn report(message: &str) {
    let mut stderr = io::stderr().lock();
    for line in message.lines() {
        let _ = writeln!(stderr, "ix-credential: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(lines: &str) -> Message {
        Message::read(&mut lines.as_bytes()).expect("parses")
    }

    fn github() -> Vec<String> {
        vec![String::from("github.com")]
    }

    /// A token file that outlives the call and dies with the test.
    struct TokenFile {
        path: std::path::PathBuf,
        /// Held only so the directory survives; dropping it deletes `path`.
        _dir: tempfile::TempDir,
    }

    fn token_file(contents: &str) -> TokenFile {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        fs::write(&path, contents).expect("write");
        TokenFile { path, _dir: dir }
    }

    #[test]
    fn a_trailing_newline_is_not_part_of_the_token() {
        let token = token_file("ghp_Token123\n");
        assert_eq!(read(&token.path).expect("readable"), "ghp_Token123");
    }

    // The shell version of this rendered `printf ... "$(cat $MISSING)"`, where
    // `set -e` discards a failed command substitution inside an argument. The
    // unit exited 0 having written an empty credential.
    #[test]
    fn a_missing_token_is_a_failure_and_not_an_empty_credential() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent");

        assert_eq!(read(&path), Err(Unusable::Missing));

        // The preflight must name the missing secret, not just fail.
        let error = check(&path).expect_err("must not pass").to_string();
        assert!(error.contains("ix secret set"), "{error}");
    }

    // And the natural self-check for that shell version, `grep -q '^password='`,
    // passed on it: git-credential-store emits a bare `password=` line for an
    // empty token, which is a present-looking field carrying no secret.
    #[test]
    fn an_empty_token_is_a_failure_rather_than_an_empty_password() {
        for contents in ["", "\n", "   \n"] {
            let token = token_file(contents);
            assert_eq!(read(&token.path), Err(Unusable::Empty), "for {contents:?}");
            let error = check(&token.path).expect_err("must not pass").to_string();
            assert!(error.contains("empty"), "for {contents:?}: {error}");
        }
    }

    #[test]
    fn a_usable_token_passes_the_preflight() {
        let token = token_file("ghp_Token123\n");
        check(&token.path).expect("usable");
    }

    // The URL rendering this replaces makes these bytes structural, so a token
    // containing one produced a credentials file matching nothing. Here they
    // are ordinary value bytes.
    #[test]
    fn a_token_containing_url_syntax_survives_intact() {
        let awkward = "tok@en:with/slash%20and?query";
        let reply = credential(awkward, "x-access-token").expect("valid");
        assert_eq!(reply.get("password"), Some(awkward));
    }

    #[test]
    fn a_token_carrying_a_line_break_is_refused_rather_than_forging_a_field() {
        let forged = "real\npassword=stolen";
        let error = credential(forged, "x-access-token")
            .expect_err("refused")
            .to_string();
        assert!(error.contains("line break"), "{error}");
    }

    #[test]
    fn answers_only_for_an_allowed_host_over_https() {
        let allowed = request("protocol=https\nhost=github.com\n\n");
        assert!(answers(&allowed, &github()));

        let cases = [
            ("another host", "protocol=https\nhost=evil.example\n\n"),
            ("cleartext", "protocol=http\nhost=github.com\n\n"),
            ("no host", "protocol=https\n\n"),
            ("no protocol", "host=github.com\n\n"),
        ];
        for (name, lines) in cases {
            assert!(!answers(&request(lines), &github()), "{name}");
        }
    }

    #[test]
    fn a_second_allowed_host_is_honored() {
        let hosts = vec![String::from("github.com"), String::from("git.example")];
        assert!(answers(
            &request("protocol=https\nhost=git.example\n\n"),
            &hosts
        ));
    }
}
