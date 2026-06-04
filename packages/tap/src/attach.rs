//! The interactive attach client.
//!
//! It runs the controlling terminal in raw mode, forwards keystrokes (minus its
//! own keybinds) to the daemon, and renders the daemon's output. Two behaviors
//! the original tap lacked: a `SIGWINCH` here sends a `Resize` so the session
//! tracks the live window, and the daemon's `Attached`/`Resized` snapshots are
//! repainted so a full-screen TUI shows up correctly on (re)attach. When the
//! negotiated session size differs from this terminal (a smaller client is
//! sharing the session), a dim warning is drawn on the bottom row.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use tap_protocol::{Request, Response};
use tap_pty::WinSize;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader, Lines, Stdout};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::signal::unix::{SignalKind, signal};

use crate::client::{self, ResolvedSocket, resolve_socket};
use crate::config;
use crate::editor;
use crate::input::{InputProcessor, InputResult, KeybindAction};
use crate::term::{self, RawGuard};

const INPUT_BUFFER_SIZE: usize = 4096;

/// The established attach connection plus the negotiated session size.
struct AttachConn {
    reader: Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
    sess_rows: u16,
    sess_cols: u16,
}

/// What to do after handling one server frame.
enum Event {
    Continue,
    Ended(i32),
}

/// Attach to a session and run until detach or session end.
///
/// On session end this exits the process with the child's exit code; on detach
/// it returns so the CLI exits cleanly.
///
/// # Errors
///
/// Returns an error if the session cannot be reached or the attach handshake
/// fails.
// The attach client owns the controlling terminal through a raw-mode guard held
// across the whole loop, so its future is intentionally thread-bound; it only
// ever runs on the main thread via the runtime's block_on, never spawned.
#[allow(clippy::future_not_send, reason = "owns the tty; runs only on the main thread")]
pub async fn run(session: Option<String>) -> Result<()> {
    let ResolvedSocket { label, socket } = resolve_socket(session)?;
    let WinSize {
        rows: mut my_rows,
        cols: mut my_cols,
    } = term::current_winsize();
    // Held for the whole session; restores the terminal on every exit path.
    let raw = RawGuard::enter();
    let mut stdout = tokio::io::stdout();

    let AttachConn {
        mut reader,
        mut writer,
        mut sess_rows,
        mut sess_cols,
    } = handshake(&socket, &label, my_rows, my_cols, &mut stdout).await?;
    draw_size_warning(my_rows, my_cols, sess_rows, sess_cols);

    let config = config::load()?;
    let mut input = InputProcessor::new(&config)?;
    let editor_cmd = config::editor_command(&config);

    let mut winch = signal(SignalKind::window_change()).context("installing SIGWINCH handler")?;
    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = [0u8; INPUT_BUFFER_SIZE];

    let mut ended: Option<i32> = None;
    let mut detached = false;

    loop {
        tokio::select! {
            line = reader.next_line() => match line {
                Ok(Some(line)) => {
                    match handle_server_frame(&line, &mut stdout, (my_rows, my_cols), (&mut sess_rows, &mut sess_cols)).await? {
                        Event::Continue => {}
                        Event::Ended(code) => {
                            ended = Some(code);
                            break;
                        }
                    }
                }
                Ok(None) | Err(_) => break,
            },
            read = stdin.read(&mut stdin_buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => match input.process(&stdin_buf[..n]) {
                    InputResult::Passthrough(bytes) => {
                        if !bytes.is_empty() {
                            send_request(&mut writer, &Request::Input { data: bytes }).await?;
                        }
                    }
                    InputResult::Action(KeybindAction::Detach) => {
                        detached = true;
                        break;
                    }
                    InputResult::Action(KeybindAction::OpenEditor) => {
                        if let Some(guard) = raw.as_ref() {
                            open_editor(&socket, &editor_cmd, guard, &mut writer, my_rows, my_cols).await;
                        }
                    }
                    InputResult::NeedMore => {}
                },
            },
            _ = winch.recv() => {
                let WinSize { rows, cols } = term::current_winsize();
                my_rows = rows;
                my_cols = cols;
                send_request(&mut writer, &Request::Resize { rows, cols }).await?;
            }
            () = tokio::time::sleep(input.escape_timeout()), if input.has_pending_escape() => {
                if let InputResult::Passthrough(bytes) = input.timeout_escape()
                    && !bytes.is_empty()
                {
                    send_request(&mut writer, &Request::Input { data: bytes }).await?;
                }
            }
        }
    }

    if detached {
        let _ = send_request(&mut writer, &Request::Detach).await;
    }
    // Restore the terminal before printing the closing line.
    drop(raw);
    if let Some(code) = ended {
        println!("[session ended]");
        std::process::exit(code);
    }
    println!("[detached]");
    Ok(())
}

/// Open the connection, send `Attach`, and paint the initial snapshot.
async fn handshake(
    socket: &Path,
    label: &str,
    my_rows: u16,
    my_cols: u16,
    stdout: &mut Stdout,
) -> Result<AttachConn> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to session '{label}'"))?;
    let (read_half, mut writer) = stream.into_split();
    let mut reader = BufReader::new(read_half).lines();

    send_request(&mut writer, &Request::Attach { rows: my_rows, cols: my_cols }).await?;
    let first = reader
        .next_line()
        .await?
        .context("session closed before attaching")?;
    let (sess_rows, sess_cols) =
        match serde_json::from_str::<Response>(&first).context("parsing attach response")? {
            Response::Attached { rows, cols, snapshot } => {
                paint(stdout, &snapshot).await?;
                (rows, cols)
            }
            Response::Error { message } => bail!("attach failed: {message}"),
            other => bail!("unexpected attach response: {other:?}"),
        };
    Ok(AttachConn { reader, writer, sess_rows, sess_cols })
}

/// Render one server frame; report whether the session has ended.
async fn handle_server_frame(
    line: &str,
    stdout: &mut Stdout,
    mine: (u16, u16),
    session: (&mut u16, &mut u16),
) -> Result<Event> {
    let Ok(response) = serde_json::from_str::<Response>(line) else {
        return Ok(Event::Continue);
    };
    match response {
        Response::Output { data } => {
            stdout.write_all(&data).await?;
            stdout.flush().await?;
        }
        Response::Resized { rows, cols, snapshot } => {
            *session.0 = rows;
            *session.1 = cols;
            paint(stdout, &snapshot).await?;
            draw_size_warning(mine.0, mine.1, rows, cols);
        }
        Response::SessionEnded { exit_code } => return Ok(Event::Ended(exit_code)),
        _ => {}
    }
    Ok(Event::Continue)
}

/// Clear the screen and paint a daemon snapshot (full screen with colors/cursor).
async fn paint(stdout: &mut Stdout, snapshot: &[u8]) -> Result<()> {
    stdout.write_all(b"\x1b[2J\x1b[H").await?;
    stdout.write_all(snapshot).await?;
    stdout.flush().await?;
    Ok(())
}

/// Draw a dim warning on the bottom row when this terminal does not match the
/// negotiated session size. Best-effort: a full-screen app may overwrite it, and
/// it is redrawn on the next resize.
fn draw_size_warning(my_rows: u16, my_cols: u16, sess_rows: u16, sess_cols: u16) {
    if (my_rows, my_cols) == (sess_rows, sess_cols) {
        return;
    }
    let detail = if my_rows < sess_rows || my_cols < sess_cols {
        "content may be clipped"
    } else {
        "extra space is unused"
    };
    let message =
        format!("[tap] terminal {my_rows}x{my_cols} != session {sess_rows}x{sess_cols} (smallest client wins); {detail}");
    let message: String = message.chars().take(my_cols as usize).collect();
    // Save cursor, go to the bottom row, dim + clear line, print, restore cursor.
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b7\x1b[{my_rows};1H\x1b[2m\x1b[K{message}\x1b[0m\x1b8");
    let _ = out.flush();
}

/// Open the scrollback in the editor, then ask the daemon to repaint.
// Holds the raw-mode guard across the scrollback fetch; like `run`, this is a
// main-thread-only future by construction.
#[allow(clippy::future_not_send, reason = "holds the tty guard; runs only on the main thread")]
async fn open_editor(
    socket: &Path,
    editor_cmd: &str,
    guard: &RawGuard,
    writer: &mut OwnedWriteHalf,
    my_rows: u16,
    my_cols: u16,
) {
    let content = client::fetch_scrollback(socket).await.unwrap_or_default();
    // The editor owns the tty while it runs; the guard restores raw mode after.
    let _ = editor::open(&content, editor_cmd, guard, None);
    // Reassert our size to make the daemon send a fresh repaint snapshot.
    let _ = send_request(writer, &Request::Resize { rows: my_rows, cols: my_cols }).await;
}

/// Write one newline-delimited JSON request frame.
async fn send_request(writer: &mut OwnedWriteHalf, request: &Request) -> Result<()> {
    let mut line = serde_json::to_string(request).context("serializing request")?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
