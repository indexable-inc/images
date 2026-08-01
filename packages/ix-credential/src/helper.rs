//! The borrowing side: a git credential helper that asks the forwarded
//! socket instead of holding anything itself.
//!
//! Registered as `credential.helper`, so git runs it whenever a remote
//! answers 401. Nothing here is ever written to disk, and the token exists
//! only in this process's memory for as long as it takes to copy it to
//! git's stdin.

use std::fs;
use std::io::{self, BufReader, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use color_eyre::eyre::Result;

use crate::protocol::{ERROR_KEY, Message};
use crate::socket;

/// A hung workstation must not hang every git command on the borrowing
/// host, so the round trip is bounded and the timeout is its own diagnostic.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Run one helper operation.
///
/// Only `get` reaches the socket. `store` and `erase` drain their input and
/// do nothing: this helper is a window onto someone else's credential, not
/// a place to keep one, and silently accepting a `store` would imply a
/// persistence it does not have.
///
/// # Errors
///
/// Only a failure to read git's own request or write its response. Every
/// failure to obtain a credential is reported on stderr and exits zero, for
/// the reason given on [`report`].
pub fn run(operation: &str) -> Result<()> {
    let request = Message::read(&mut BufReader::new(io::stdin().lock()))?;
    if operation != "get" {
        return Ok(());
    }

    let path = socket::path();
    let reply = match fetch(&path, &request) {
        Ok(reply) => reply,
        Err(failure) => {
            report(&failure.describe(&path));
            return Ok(());
        }
    };

    if let Some(reason) = reply.get(ERROR_KEY) {
        report(&format!(
            "the credential agent on your workstation refused: {reason}"
        ));
        return Ok(());
    }

    reply.write(&mut io::stdout().lock())
}

/// Why a round trip did not produce a credential.
///
/// git sees one outcome, "no credential", for all of these. The operator
/// needs to tell them apart, because the thing to do next differs: start a
/// loan, reconnect one, or go look at what is sitting on the path. So they
/// are variants with their own text rather than one generic failure.
#[derive(Debug)]
enum Failure {
    /// Nothing is lending. The usual state of a host with no operator on it.
    NoLoan,
    /// The path is there but nothing is listening: a lending session died
    /// without unlinking. sshd is configured to clear these on the next
    /// bind, so this survives only until the operator reconnects.
    Stale,
    /// Something that is not a socket is sitting on the path. Not a dead
    /// loan, so reconnecting will not fix it; someone has to look.
    NotASocket,
    /// The socket answered, but the conversation failed.
    Broken(String),
}

impl Failure {
    fn describe(&self, path: &Path) -> String {
        let path = path.display();
        match self {
            Self::NoLoan => format!(
                "no credential loan is running: {path} does not exist.\n\
                 Lend one from your workstation with:\n    \
                 ix-credential lend <this-host>"
            ),
            Self::Stale => format!(
                "stale credential socket at {path}: the socket is there but nothing is \
                 listening, so the lending session ended without cleaning up.\n\
                 Reconnect the loan; the next bind replaces the stale socket."
            ),
            Self::NotASocket => format!(
                "{path} exists but is not a socket, so no loan can be bound there.\n\
                 Something else is using the path; move it aside before lending."
            ),
            Self::Broken(detail) => format!(
                "credential loan at {path} is not answering: {detail}\n\
                 Check the `ix-credential serve` process on your workstation."
            ),
        }
    }
}

/// One round trip to the lending socket.
fn fetch(path: &Path, request: &Message) -> Result<Message, Failure> {
    if !path.exists() {
        return Err(Failure::NoLoan);
    }

    let mut stream = UnixStream::connect(path).map_err(|error| {
        // Which failure this is cannot come from the errno, because the two
        // platforms disagree. Linux answers ECONNREFUSED for both a dead
        // socket and an ordinary file; macOS separates them with ENOTSOCK.
        // Keying on ENOTSOCK therefore classified every non-socket on Linux
        // as a stale loan and told the operator to reconnect, which cannot
        // help when the path holds a regular file.
        //
        // Ask the inode what it is instead. stat still cannot tell a live
        // socket from a stale one, which is what the connect above is for,
        // but it does tell a socket from anything else, identically on both
        // platforms.
        let not_a_socket = fs::metadata(path).is_ok_and(|meta| !meta.file_type().is_socket());
        if not_a_socket {
            Failure::NotASocket
        } else if error.kind() == io::ErrorKind::ConnectionRefused {
            Failure::Stale
        } else {
            Failure::Broken(error.to_string())
        }
    })?;

    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|error| Failure::Broken(error.to_string()))?;

    request
        .write(&mut stream)
        .map_err(|error| Failure::Broken(format!("sending the request failed: {error}")))?;

    Message::read(&mut BufReader::new(stream))
        .map_err(|error| Failure::Broken(format!("reading the reply failed: {error}")))
}

/// Say why on stderr, then let git carry on and fail its own way.
///
/// Exiting nonzero here would be the louder choice, and it is the wrong one:
/// this helper is registered for every git operation on the host, so a
/// failure would turn "no loan is running" into "git is broken for
/// everybody". git only consults a credential helper once a remote has
/// already demanded authentication, so this message appears exactly when
/// the operator was about to see an authentication failure anyway, and it
/// explains it.
fn report(message: &str) {
    let mut stderr = io::stderr().lock();
    for line in message.lines() {
        let _ = writeln!(stderr, "ix-credential: {line}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn a_missing_socket_reads_as_no_loan_and_says_how_to_start_one() {
        let path = PathBuf::from("/nonexistent/ix-credential/0.sock");
        let request = Message::default();

        let failure = fetch(&path, &request).expect_err("no socket");
        assert!(matches!(failure, Failure::NoLoan), "{failure:?}");

        let text = failure.describe(&path);
        assert!(text.contains("no credential loan is running"), "{text}");
        assert!(text.contains("ix-credential lend"), "{text}");
    }

    #[test]
    fn a_socket_whose_listener_is_gone_reads_as_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("stale.sock");
        // Exactly what a dead lending session leaves: dropping the listener
        // closes the descriptor but does not unlink the path, so the inode
        // outlives the process that bound it.
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        drop(listener);

        let failure = fetch(&path, &Message::default()).expect_err("nothing listening");
        assert!(matches!(failure, Failure::Stale), "{failure:?}");
        let text = failure.describe(&path);
        assert!(text.contains("stale credential socket"), "{text}");
    }

    #[test]
    fn an_ordinary_file_on_the_path_is_not_reported_as_a_dead_loan() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-a-socket");
        std::fs::write(&path, b"").expect("create");

        let failure = fetch(&path, &Message::default()).expect_err("not connectable");
        assert!(matches!(failure, Failure::NotASocket), "{failure:?}");
        let text = failure.describe(&path);
        assert!(text.contains("is not a socket"), "{text}");
        // Reconnecting cannot fix this, so the message must not suggest it.
        assert!(!text.contains("Reconnect"), "{text}");
    }

    #[test]
    fn every_failure_gets_its_own_message() {
        let path = Path::new("/run/ix-credential/0.sock");
        let messages = [
            Failure::NoLoan.describe(path),
            Failure::Stale.describe(path),
            Failure::NotASocket.describe(path),
            Failure::Broken("connection reset".into()).describe(path),
        ];

        for (index, first) in messages.iter().enumerate() {
            for second in &messages[index + 1..] {
                assert_ne!(first, second);
            }
        }
    }
}
