//! The lending side: a socket on the operator's workstation that answers
//! one question, "what credential for this host", by asking whatever
//! already knows.
//!
//! This is agent forwarding's shape applied to a token. The workstation
//! keeps the secret and performs the lookup; the borrowing host gets an
//! answer over a socket whose lifetime is the ssh session's. Nothing is
//! copied, so nothing has to be cleaned up, and revoking the loan is
//! hanging up.

use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use color_eyre::eyre::{Context, Result, bail};

use crate::protocol::Message;

/// A borrower that connects and then says nothing must not tie up a thread
/// forever.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Answers "what credential for this host".
pub trait Resolver: Send + Sync {
    /// `Ok(None)` means the resolver works and has nothing for this host,
    /// which is a different answer from `Err`, which means it could not
    /// look. The borrower is told which.
    fn resolve(&self, host: &str) -> Result<Option<String>, String>;
}

/// The default resolver: whatever `gh` is already logged in as.
///
/// Deliberately not a configurable command. A resolver read from a config
/// file would make that file a way to run arbitrary code as the operator,
/// which is a worse hole than the one this closes.
pub struct GhResolver;

impl Resolver for GhResolver {
    fn resolve(&self, host: &str) -> Result<Option<String>, String> {
        let output = Command::new("gh")
            .args(["auth", "token", "--hostname", host])
            .output()
            .map_err(|error| format!("could not run `gh`: {error}"))?;

        if !output.status.success() {
            // gh says "no oauth token" on a host it is not logged in to.
            // That is "nothing for this host", not a broken resolver.
            return Ok(None);
        }

        let token = String::from_utf8(output.stdout)
            .map_err(|_| "`gh auth token` printed something that is not text".to_owned())?;
        let token = token.trim();

        if token.is_empty() {
            Ok(None)
        } else {
            Ok(Some(token.to_owned()))
        }
    }
}

/// Serve until killed.
///
/// # Errors
///
/// The socket cannot be bound, or another agent already holds it.
pub fn run(path: &Path, allowed: &[String], resolver: &dyn Resolver) -> Result<()> {
    let listener = bind(path)?;
    eprintln!(
        "ix-credential: lending on {} for {}",
        path.display(),
        allowed.join(", ")
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => serve_one(stream, allowed, resolver),
            Err(error) => eprintln!("ix-credential: accept failed: {error}"),
        }
    }
    Ok(())
}

/// Bind the socket, replacing one a dead agent left behind.
///
/// A path that exists is ambiguous: it is either a live agent or a corpse.
/// Connecting is the only way to tell, so that is what this does, and the
/// two answers get two outcomes rather than one blind unlink that could
/// steal a running agent's socket.
fn bind(path: &Path) -> Result<UnixListener> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .wrap_err_with(|| format!("creating {}", parent.display()))?;
        // Only meaningful where we own the directory. On a shared host the
        // directory is 1777 and provisioned by the host configuration.
        if let Ok(metadata) = fs::metadata(parent)
            && metadata.permissions().mode() & 0o777 == 0o755
        {
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!(
                "another credential agent is already lending on {}",
                path.display()
            );
        }
        fs::remove_file(path)
            .wrap_err_with(|| format!("removing the stale socket at {}", path.display()))?;
    }

    let listener = UnixListener::bind(path)
        .wrap_err_with(|| format!("binding {}", path.display()))?;
    // The umask decides the mode at bind time, so set it explicitly rather
    // than inheriting whatever the operator's shell happens to have.
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .wrap_err_with(|| format!("restricting {}", path.display()))?;
    Ok(listener)
}

/// Handle one request inline.
///
/// One connection is one short question, and git asks them one at a time,
/// so a thread per connection would buy nothing but a way to interleave the
/// log.
fn serve_one(stream: UnixStream, allowed: &[String], resolver: &dyn Resolver) {
    if let Err(error) = exchange(stream, allowed, resolver) {
        eprintln!("ix-credential: request failed: {error}");
    }
}

fn exchange(mut stream: UnixStream, allowed: &[String], resolver: &dyn Resolver) -> Result<()> {
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;

    let request = Message::read(&mut BufReader::new(stream.try_clone()?))?;
    let reply = answer(&request, allowed, resolver)?;
    reply.write(&mut stream)?;
    stream.flush()?;
    Ok(())
}

/// Decide what to say. Separated from the socket so the whole policy is
/// testable by handing it a message.
fn answer(request: &Message, allowed: &[String], resolver: &dyn Resolver) -> Result<Message> {
    let Some(host) = request.get("host") else {
        return log_refusal("<none>", "the request named no host");
    };

    // The host reaches `gh` as an argument, and the request comes from a
    // machine we are lending to rather than one we trust. Anything outside
    // a hostname's alphabet, and anything that could read as an option, is
    // refused before it becomes argv.
    if !is_hostname(host) {
        return log_refusal(host, "the requested host is not a hostname");
    }

    let protocol = request.get("protocol").unwrap_or("");
    if protocol != "https" {
        return log_refusal(host, &format!("protocol {protocol:?} is not https"));
    }

    if !allowed.iter().any(|candidate| candidate == host) {
        return log_refusal(host, "this host is not on the agent's allow list");
    }

    match resolver.resolve(host) {
        Err(detail) => log_refusal(host, &detail),
        Ok(None) => log_refusal(host, "the workstation has no credential for it"),
        Ok(Some(token)) => {
            eprintln!("ix-credential: host={host} -> lent");
            let mut reply = Message::default();
            // GitHub accepts any username alongside a token over basic auth
            // and documents this one for app tokens.
            reply.push("username", "x-access-token")?;
            reply.push("password", &token)?;
            Ok(reply)
        }
    }
}

/// Every refusal is logged with its host and its reason, and no refusal
/// path can reach a token, so the log cannot leak one.
fn log_refusal(host: &str, reason: &str) -> Result<Message> {
    eprintln!("ix-credential: host={host} -> refused: {reason}");
    Message::refusal(reason)
}

/// A conservative hostname: letters, digits, dot and hyphen, not starting
/// with a hyphen, and not empty.
fn is_hostname(candidate: &str) -> bool {
    !candidate.is_empty()
        && !candidate.starts_with('-')
        && candidate.len() <= 253
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ERROR_KEY;

    struct Fixed(Option<&'static str>);

    impl Resolver for Fixed {
        fn resolve(&self, _host: &str) -> Result<Option<String>, String> {
            Ok(self.0.map(ToOwned::to_owned))
        }
    }

    struct Broken;

    impl Resolver for Broken {
        fn resolve(&self, _host: &str) -> Result<Option<String>, String> {
            Err("could not run `gh`: No such file or directory".to_owned())
        }
    }

    fn allowed() -> Vec<String> {
        vec!["github.com".to_owned()]
    }

    fn request(pairs: &[(&str, &str)]) -> Message {
        let mut message = Message::default();
        for (key, value) in pairs {
            message.push(key, value).expect("valid");
        }
        message
    }

    fn github_get() -> Message {
        request(&[("protocol", "https"), ("host", "github.com")])
    }

    #[test]
    fn a_known_host_gets_the_token() {
        let reply = answer(&github_get(), &allowed(), &Fixed(Some("ghp_example"))).expect("answers");
        assert_eq!(reply.get("username"), Some("x-access-token"));
        assert_eq!(reply.get("password"), Some("ghp_example"));
        assert_eq!(reply.get(ERROR_KEY), None);
    }

    #[test]
    fn no_credential_is_its_own_refusal_not_a_resolver_error() {
        let reply = answer(&github_get(), &allowed(), &Fixed(None)).expect("answers");
        assert_eq!(reply.get(ERROR_KEY), Some("the workstation has no credential for it"));
        assert_eq!(reply.get("password"), None);
    }

    #[test]
    fn a_resolver_that_cannot_look_says_so() {
        let reply = answer(&github_get(), &allowed(), &Broken).expect("answers");
        let error = reply.get(ERROR_KEY).expect("refused");
        assert!(error.contains("could not run `gh`"), "{error}");
    }

    #[test]
    fn a_host_off_the_allow_list_is_refused_without_consulting_the_resolver() {
        let message = request(&[("protocol", "https"), ("host", "evil.example")]);
        let reply = answer(&message, &allowed(), &Fixed(Some("ghp_example"))).expect("answers");
        assert_eq!(reply.get("password"), None);
        assert!(
            reply.get(ERROR_KEY).is_some_and(|e| e.contains("allow list")),
            "{reply:?}"
        );
    }

    #[test]
    fn a_non_https_protocol_is_refused() {
        let message = request(&[("protocol", "http"), ("host", "github.com")]);
        let reply = answer(&message, &allowed(), &Fixed(Some("ghp_example"))).expect("answers");
        assert_eq!(reply.get("password"), None);
    }

    #[test]
    fn a_host_that_could_read_as_an_option_never_reaches_argv() {
        for hostile in [
            "--version",
            "-oProxyCommand=x",
            "github.com;id",
            "github.com/../evil",
            "",
        ] {
            let message = request(&[("protocol", "https"), ("host", hostile)]);
            let reply = answer(&message, &allowed(), &Fixed(Some("ghp_example"))).expect("answers");
            assert_eq!(reply.get("password"), None, "leaked for {hostile:?}");
        }
    }

    #[test]
    fn a_request_with_no_host_is_refused() {
        let message = request(&[("protocol", "https")]);
        let reply = answer(&message, &allowed(), &Fixed(Some("ghp_example"))).expect("answers");
        assert_eq!(reply.get("password"), None);
    }

    #[test]
    fn hostnames_are_recognised_conservatively() {
        assert!(is_hostname("github.com"));
        assert!(is_hostname("git.internal.example-corp.com"));
        assert!(!is_hostname("-github.com"));
        assert!(!is_hostname("github.com "));
        assert!(!is_hostname("git@github.com"));
        assert!(!is_hostname(""));
    }

    #[test]
    fn binding_refuses_to_steal_a_live_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("live.sock");

        let _held = bind(&path).expect("first bind");
        let error = bind(&path).expect_err("second bind");
        assert!(
            format!("{error}").contains("already lending"),
            "{error}"
        );
    }

    #[test]
    fn binding_replaces_a_socket_no_one_is_listening_on() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stale.sock");
        std::fs::write(&path, b"").expect("corpse");

        let listener = bind(&path).expect("rebinds over the corpse");
        drop(listener);
    }

    #[test]
    fn the_bound_socket_is_not_readable_by_anyone_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("perms.sock");

        let _listener = bind(&path).expect("bind");
        let mode = fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "socket mode {mode:o}");
    }

    #[test]
    fn a_full_round_trip_over_a_real_socket_returns_the_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("round-trip.sock");
        let listener = bind(&path).expect("bind");

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            exchange(stream, &allowed(), &Fixed(Some("ghp_round_trip"))).expect("exchange");
        });

        let mut client = UnixStream::connect(&path).expect("connect");
        github_get().write(&mut client).expect("send");
        let reply = Message::read(&mut BufReader::new(client)).expect("reply");

        server.join().expect("server thread");
        assert_eq!(reply.get("password"), Some("ghp_round_trip"));
    }
}
