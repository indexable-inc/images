//! Client commands: starting sessions, listing them, and one-shot control
//! queries. The interactive attach loop lives in [`crate::attach`].

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail};
use tap_protocol::{Request, Response};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;

use crate::{attach, index, names};

const SHELL_FALLBACK: &str = "/bin/sh";
/// How long to wait for a freshly spawned daemon to bind its socket.
const DAEMON_STARTUP_POLLS: u32 = 300;
const DAEMON_STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Start a session and either attach to it or, when `detached`, return.
///
/// # Errors
///
/// Returns an error if the daemon cannot be spawned or, in the attached case, if
/// attach fails.
// Awaits the attach client, whose future is thread-bound by design; this runs
// only on the main thread via the runtime's block_on.
#[allow(clippy::future_not_send, reason = "delegates to the main-thread attach loop")]
pub async fn start(command: Vec<String>, detached: bool, id: Option<String>) -> Result<()> {
    let session_id = id.unwrap_or_else(names::generate);
    let argv = resolve_command(command);
    spawn_daemon(&session_id, &argv).await?;

    if detached {
        println!("started session {session_id}");
        println!("attach with: tap attach {session_id}");
        Ok(())
    } else {
        attach::run(Some(session_id)).await
    }
}

/// Resolve the command to run, defaulting to an interactive `$SHELL`.
fn resolve_command(command: Vec<String>) -> Vec<String> {
    if !command.is_empty() {
        return command;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| SHELL_FALLBACK.to_string());
    // Force interactive/login mode so the shell loads its config in the session.
    if shell.ends_with("/nu") || shell.ends_with("/nushell") {
        vec![shell, "-l".to_string()]
    } else if shell.ends_with("/bash") || shell.ends_with("/zsh") {
        vec![shell, "-i".to_string()]
    } else {
        vec![shell]
    }
}

/// Spawn the session daemon detached from the controlling terminal and wait for
/// its socket to appear.
async fn spawn_daemon(id: &str, argv: &[String]) -> Result<PathBuf> {
    use std::process::Stdio;

    let exe = std::env::current_exe().context("locating the tap executable")?;
    let runtime_dir = tap_protocol::runtime_dir();
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("creating runtime dir {}", runtime_dir.display()))?;
    let socket = tap_protocol::socket_path(id);
    let _ = std::fs::remove_file(&socket);

    // Daemon diagnostics land in a per-session log so a failed start is not
    // silent; stdin/stdout are detached from the terminal.
    let log = std::fs::File::create(runtime_dir.join(format!("{id}.log")))
        .with_context(|| format!("creating daemon log for session '{id}'"))?;

    let mut command = std::process::Command::new(exe);
    command
        .arg("daemon")
        .arg("--id")
        .arg(id)
        .arg("--socket")
        .arg(&socket)
        .arg("--")
        .args(argv);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    // The daemon detaches itself with setsid *after* spawning its PTY child (see
    // `daemon::run`): macOS rejects the child's TIOCSCTTY when the parent is
    // already a session leader, so we must not setsid before the child spawns.
    command.spawn().context("spawning session daemon")?;

    for _ in 0..DAEMON_STARTUP_POLLS {
        if socket.exists() {
            return Ok(socket);
        }
        tokio::time::sleep(DAEMON_STARTUP_POLL_INTERVAL).await;
    }
    bail!("session daemon did not come up in time")
}

/// A resolved session: its id (`label`) and the path to its Unix socket.
pub struct ResolvedSocket {
    /// Human-readable session id.
    pub label: String,
    /// Path to the session's Unix socket.
    pub socket: PathBuf,
}

/// Resolve a session selector to its id and socket, defaulting to the most
/// recently started live session.
///
/// # Errors
///
/// Returns an error if the named session is missing or there are no sessions.
pub fn resolve_socket(session: Option<String>) -> Result<ResolvedSocket> {
    let Some(id) = session else {
        let latest =
            index::latest_live().ok_or_else(|| anyhow!("no active sessions; start one with `tap`"))?;
        return Ok(ResolvedSocket {
            label: latest.id,
            socket: latest.socket,
        });
    };
    let socket = tap_protocol::socket_path(&id);
    if socket.exists() {
        Ok(ResolvedSocket { label: id, socket })
    } else {
        bail!("session '{id}' not found; run `tap list`")
    }
}

/// Print the live sessions as a table.
pub fn list() {
    let sessions = index::list_live();
    if sessions.is_empty() {
        println!("No active sessions");
        return;
    }
    println!("{:<24} {:<8} {:<12} COMMAND", "ID", "PID", "STARTED");
    for session in sessions {
        println!(
            "{:<24} {:<8} {:<12} {}",
            session.id,
            session.pid,
            format_started(session.started_unix),
            session.command.join(" ")
        );
    }
}

/// Print the session's screen as plain text.
///
/// # Errors
///
/// Returns an error if the session cannot be reached.
pub async fn scrollback(session: Option<String>, lines: Option<usize>) -> Result<()> {
    let ResolvedSocket { socket, .. } = resolve_socket(session)?;
    match request_once(&socket, &Request::GetScrollback { lines }).await? {
        Response::Scrollback { content } => {
            print!("{content}");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Print the cursor position.
///
/// # Errors
///
/// Returns an error if the session cannot be reached.
pub async fn cursor(session: Option<String>) -> Result<()> {
    let ResolvedSocket { socket, .. } = resolve_socket(session)?;
    match request_once(&socket, &Request::GetCursor).await? {
        Response::Cursor { row, col } => {
            println!("row {row}, col {col}");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Print the negotiated session size.
///
/// # Errors
///
/// Returns an error if the session cannot be reached.
pub async fn size(session: Option<String>) -> Result<()> {
    let ResolvedSocket { socket, .. } = resolve_socket(session)?;
    match request_once(&socket, &Request::GetSize).await? {
        Response::Size { rows, cols } => {
            println!("{rows}x{cols}");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Inject text into the session's child without attaching.
///
/// # Errors
///
/// Returns an error if the session cannot be reached or rejects the write.
pub async fn inject(session: Option<String>, text: String) -> Result<()> {
    let ResolvedSocket { socket, .. } = resolve_socket(session)?;
    match request_once(&socket, &Request::Inject { data: text }).await? {
        Response::Ok => {
            println!("injected");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Terminate a session: kill its child and shut down its daemon.
///
/// # Errors
///
/// Returns an error if the session cannot be reached or rejects the request.
pub async fn kill(session: Option<String>) -> Result<()> {
    let ResolvedSocket { label: id, socket } = resolve_socket(session)?;
    match request_once(&socket, &Request::Kill).await? {
        Response::Ok => {
            println!("killed session {id}");
            Ok(())
        }
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Stream the session's raw output to stdout until it ends.
///
/// # Errors
///
/// Returns an error if the session cannot be reached.
pub async fn subscribe(session: Option<String>) -> Result<()> {
    let ResolvedSocket { socket, .. } = resolve_socket(session)?;
    let stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    write_request(&mut write_half, &Request::Subscribe).await?;

    let mut reader = BufReader::new(read_half).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = reader.next_line().await? {
        let Ok(response) = serde_json::from_str::<Response>(&line) else {
            continue;
        };
        match response {
            Response::Output { data } => {
                stdout.write_all(&data).await?;
                stdout.flush().await?;
            }
            Response::SessionEnded { .. } => break,
            _ => {}
        }
    }
    Ok(())
}

/// Fetch the session's full screen as text over a fresh connection.
///
/// # Errors
///
/// Returns an error if the session cannot be reached or responds unexpectedly.
pub async fn fetch_scrollback(socket: &Path) -> Result<String> {
    match request_once(socket, &Request::GetScrollback { lines: None }).await? {
        Response::Scrollback { content } => Ok(content),
        Response::Error { message } => bail!("{message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Connect, send one request, and read one response.
async fn request_once(socket: &Path, request: &Request) -> Result<Response> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    write_request(&mut write_half, request).await?;

    let mut line = String::new();
    BufReader::new(read_half).read_line(&mut line).await?;
    if line.is_empty() {
        bail!("session closed the connection without responding");
    }
    serde_json::from_str(&line).context("parsing response")
}

/// Write one newline-delimited JSON request frame.
async fn write_request(writer: &mut OwnedWriteHalf, request: &Request) -> Result<()> {
    let mut line = serde_json::to_string(request).context("serializing request")?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Render a start time as a short relative string for `tap list`.
fn format_started(started_unix: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let secs = now.saturating_sub(started_unix);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}
