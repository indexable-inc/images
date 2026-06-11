//! The session daemon: owns one [`PtySession`], serves clients over a Unix
//! socket, and negotiates a shared screen size for multiplayer attach.
//!
//! The daemon is the only model in tap: every interactive `tap` (and `tap
//! attach`) is a client of a daemon, even a single user. That uniformity is what
//! makes resize-while-attached and multi-client sharing fall out for free, where
//! the original tap embedded the server in the foreground process and could
//! neither resize a second attacher nor admit one.
//!
//! Size negotiation is element-wise min over all attached clients (tmux's rule):
//! the session is as large as the smallest participant so no client sees clipped
//! output, and every other client is told the negotiated size so it can warn
//! that part of its terminal is unused.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use parking_lot::Mutex;

use crate::index;
use tap_protocol::{Request, Response, Session};
use tap_pty::{Attachment, CursorPosition, PtySession, SessionConfig, WinSize};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const SCROLLBACK_LINES: usize = 10_000;
/// Time to let client writers flush `SessionEnded` before the socket is removed.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(50);

/// A repaint pushed to one attached client when the negotiated size changes.
struct ResizeNotice {
    rows: u16,
    cols: u16,
    snapshot: Vec<u8>,
}

/// Per-client bookkeeping for size negotiation and out-of-band repaints.
struct ClientReg {
    control: mpsc::UnboundedSender<ResizeNotice>,
    rows: u16,
    cols: u16,
    /// Attached clients drive size negotiation and send input; observers (from
    /// `Subscribe`) only read output.
    attached: bool,
}

struct Inner {
    next_id: u64,
    session_size: WinSize,
    clients: HashMap<u64, ClientReg>,
}

/// The result of attaching an interactive client: its registry id, the
/// negotiated session size, and the live output attachment.
struct AttachResult {
    /// The new client's registry id.
    id: u64,
    /// The negotiated session size after this client joined.
    size: WinSize,
    /// The live output stream plus resync snapshot.
    attachment: Attachment,
}

/// The result of registering a read-only observer: its registry id and the live
/// output attachment.
struct ObserverResult {
    /// The new observer's registry id.
    id: u64,
    /// The live output stream plus resync snapshot.
    attachment: Attachment,
}

/// Shared daemon state: the PTY session plus the client registry.
struct DaemonState {
    session: PtySession,
    inner: Mutex<Inner>,
}

impl DaemonState {
    fn new(session: PtySession, size: WinSize) -> Self {
        Self {
            session,
            inner: Mutex::new(Inner {
                next_id: 0,
                session_size: size,
                clients: HashMap::new(),
            }),
        }
    }

    fn session_size(&self) -> WinSize {
        self.inner.lock().session_size
    }

    /// Register an attached client, renegotiate size, and start its output feed.
    fn attach(
        &self,
        rows: u16,
        cols: u16,
        control: mpsc::UnboundedSender<ResizeNotice>,
    ) -> AttachResult {
        let mut inner = self.inner.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.clients.insert(
            id,
            ClientReg {
                control,
                rows,
                cols,
                attached: true,
            },
        );
        // Exclude this client: it learns its size from the Attached response,
        // not a Resized notice.
        self.recompute(&mut inner, Some(id));
        let size = inner.session_size;
        let attachment = self.session.subscribe();
        drop(inner);
        AttachResult {
            id,
            size,
            attachment,
        }
    }

    /// Register a read-only observer that does not affect size negotiation.
    fn add_observer(&self, control: mpsc::UnboundedSender<ResizeNotice>) -> ObserverResult {
        let mut inner = self.inner.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        inner.clients.insert(
            id,
            ClientReg {
                control,
                rows: 0,
                cols: 0,
                attached: false,
            },
        );
        let attachment = self.session.subscribe();
        drop(inner);
        ObserverResult { id, attachment }
    }

    /// Record a client's new terminal size, renegotiate, and repaint it.
    fn update_size(&self, id: u64, rows: u16, cols: u16) {
        let mut inner = self.inner.lock();
        if let Some(client) = inner.clients.get_mut(&id) {
            client.rows = rows;
            client.cols = cols;
        }
        self.recompute(&mut inner, None);
        // Always hand the requester a fresh snapshot so it repaints on its own
        // resize (and after the editor keybind, which fakes a resize to refresh).
        let WinSize {
            rows: s_rows,
            cols: s_cols,
        } = inner.session_size;
        let snapshot = self.session.snapshot();
        if let Some(client) = inner.clients.get(&id) {
            let _ = client.control.send(ResizeNotice {
                rows: s_rows,
                cols: s_cols,
                snapshot,
            });
        }
        drop(inner);
    }

    fn remove(&self, id: u64) {
        let mut inner = self.inner.lock();
        inner.clients.remove(&id);
        self.recompute(&mut inner, None);
        drop(inner);
    }

    /// Set the session size to the element-wise min over attached clients. On a
    /// change, resize the PTY and push a repaint to every attached client except
    /// `exclude`.
    fn recompute(&self, inner: &mut Inner, exclude: Option<u64>) {
        let sizes: Vec<(u16, u16)> = inner
            .clients
            .values()
            .filter(|c| c.attached)
            .map(|c| (c.rows, c.cols))
            .collect();
        if sizes.is_empty() {
            return;
        }
        let rows = sizes
            .iter()
            .map(|s| s.0)
            .min()
            .unwrap_or(DEFAULT_ROWS)
            .max(1);
        let cols = sizes
            .iter()
            .map(|s| s.1)
            .min()
            .unwrap_or(DEFAULT_COLS)
            .max(1);
        let new = WinSize { rows, cols };
        if new == inner.session_size {
            return;
        }

        let _ = self.session.resize(rows, cols);
        inner.session_size = new;
        let snapshot = self.session.snapshot();
        for (client_id, client) in &inner.clients {
            if !client.attached || Some(*client_id) == exclude {
                continue;
            }
            let _ = client.control.send(ResizeNotice {
                rows,
                cols,
                snapshot: snapshot.clone(),
            });
        }
    }
}

/// Run the session daemon until the child exits.
///
/// # Errors
///
/// Returns an error if the PTY child cannot be spawned or the socket cannot bind.
pub async fn run(id: String, socket: PathBuf, command: Vec<String>) -> Result<()> {
    let runtime_dir = tap_protocol::runtime_dir();
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("creating runtime dir {}", runtime_dir.display()))?;

    let session = PtySession::spawn(SessionConfig {
        command: command.clone(),
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        scrollback_lines: SCROLLBACK_LINES,
    })
    .context("spawning session child")?;

    // Detach from the launching client's session so closing the client does not
    // SIGHUP the daemon. This runs after the child spawns on purpose: macOS
    // rejects the child's TIOCSCTTY when its parent is already a session leader.
    // Best-effort: failure (EPERM if already a leader) leaves the daemon usable.
    if let Err(error) = nix::unistd::setsid() {
        eprintln!("tap daemon: setsid failed ({error}); session may not survive client exit");
    }

    let started_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    index::add(Session {
        id: id.clone(),
        pid: std::process::id(),
        started_unix,
        command,
        socket: socket.clone(),
    })
    .context("recording session in index")?;

    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("binding socket {}", socket.display()))?;

    let state = Arc::new(DaemonState::new(
        session,
        WinSize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
        },
    ));
    let mut exit = state.session.exit_watch();

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _addr)) = accepted {
                    let state = Arc::clone(&state);
                    tokio::spawn(async move {
                        let _ = handle_conn(stream, state).await;
                    });
                }
            }
            changed = exit.changed() => {
                if changed.is_err() || state.session.exit_code().is_some() {
                    break;
                }
            }
        }
    }

    // The child exited. Client writers send SessionEnded off their own exit
    // watch; give them a moment to flush before the socket disappears.
    tokio::time::sleep(SHUTDOWN_GRACE).await;
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(runtime_dir.join(format!("{id}.log")));
    let _ = index::remove(&id);
    Ok(())
}

/// Handle one client connection: dispatch control queries, or hand the
/// connection to the attach/subscribe streaming paths.
async fn handle_conn(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        let Ok(request) = serde_json::from_str::<Request>(&line) else {
            continue;
        };
        match request {
            Request::Attach { rows, cols } => {
                return handle_attach(state, lines, write_half, rows, cols).await;
            }
            Request::Subscribe => return handle_subscribe(state, write_half).await,
            Request::GetScrollback { lines: count } => {
                let content = state.session.scrollback(count);
                send_line(&mut write_half, &Response::Scrollback { content }).await?;
            }
            Request::GetCursor => {
                let CursorPosition { row, col } = state.session.cursor();
                send_line(&mut write_half, &Response::Cursor { row, col }).await?;
            }
            Request::GetSize => {
                let WinSize { rows, cols } = state.session_size();
                send_line(&mut write_half, &Response::Size { rows, cols }).await?;
            }
            Request::Inject { data } => {
                let _ = state.session.write_input(data.into_bytes());
                send_line(&mut write_half, &Response::Ok).await?;
            }
            Request::Kill => {
                let _ = state.session.kill();
                send_line(&mut write_half, &Response::Ok).await?;
            }
            Request::Input { .. } | Request::Resize { .. } | Request::Detach => {
                let message = "request only valid on an attached connection".to_string();
                send_line(&mut write_half, &Response::Error { message }).await?;
            }
        }
    }
    Ok(())
}

/// Drive an attached client: stream output and resizes out, take input back in.
async fn handle_attach(
    state: Arc<DaemonState>,
    mut lines: Lines<BufReader<OwnedReadHalf>>,
    mut write_half: OwnedWriteHalf,
    rows: u16,
    cols: u16,
) -> Result<()> {
    let (control_tx, mut control_rx) = mpsc::unbounded_channel();
    let AttachResult {
        id,
        size: WinSize {
            rows: s_rows,
            cols: s_cols,
        },
        attachment,
    } = state.attach(rows, cols, control_tx);
    send_line(
        &mut write_half,
        &Response::Attached {
            rows: s_rows,
            cols: s_cols,
            snapshot: attachment.snapshot,
        },
    )
    .await?;

    let mut output = attachment.output;
    let mut exit = state.session.exit_watch();
    let lag_state = Arc::clone(&state);

    let writer = tokio::spawn(async move {
        // Copy out so the watch guard does not cross the await below.
        let already_exited = *exit.borrow();
        if let Some(code) = already_exited {
            let _ = send_line(&mut write_half, &Response::SessionEnded { exit_code: code }).await;
            return;
        }
        loop {
            tokio::select! {
                biased;
                notice = control_rx.recv() => {
                    let Some(notice) = notice else { break };
                    let resized = Response::Resized { rows: notice.rows, cols: notice.cols, snapshot: notice.snapshot };
                    if send_line(&mut write_half, &resized).await.is_err() {
                        break;
                    }
                }
                received = output.recv() => match received {
                    Ok(bytes) => {
                        if send_line(&mut write_half, &Response::Output { data: bytes.to_vec() }).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => {
                        // Fell behind: resync from a fresh snapshot instead of
                        // replaying a torn byte stream.
                        let WinSize { rows: r, cols: c } = lag_state.session_size();
                        let snapshot = lag_state.session.snapshot();
                        let resized = Response::Resized { rows: r, cols: c, snapshot };
                        if send_line(&mut write_half, &resized).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Closed) => break,
                },
                changed = exit.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let exited = *exit.borrow();
                    if let Some(code) = exited {
                        let _ = send_line(&mut write_half, &Response::SessionEnded { exit_code: code }).await;
                        break;
                    }
                }
            }
        }
    });

    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(request) = serde_json::from_str::<Request>(&line) else {
            continue;
        };
        match request {
            Request::Input { data } => {
                let _ = state.session.write_input(data);
            }
            Request::Resize { rows, cols } => state.update_size(id, rows, cols),
            Request::Detach => break,
            _ => {}
        }
    }

    state.remove(id);
    writer.abort();
    Ok(())
}

/// Drive a read-only observer: an initial paint, then the live output stream.
async fn handle_subscribe(state: Arc<DaemonState>, mut write_half: OwnedWriteHalf) -> Result<()> {
    let (control_tx, _control_rx) = mpsc::unbounded_channel();
    let ObserverResult { id, attachment } = state.add_observer(control_tx);
    send_line(&mut write_half, &Response::Subscribed).await?;
    send_line(
        &mut write_half,
        &Response::Output {
            data: attachment.snapshot,
        },
    )
    .await?;

    let mut output = attachment.output;
    let mut exit = state.session.exit_watch();
    loop {
        tokio::select! {
            received = output.recv() => match received {
                Ok(bytes) => {
                    if send_line(&mut write_half, &Response::Output { data: bytes.to_vec() }).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            changed = exit.changed() => {
                if changed.is_err() {
                    break;
                }
                let exited = *exit.borrow();
                if let Some(code) = exited {
                    let _ = send_line(&mut write_half, &Response::SessionEnded { exit_code: code }).await;
                    break;
                }
            }
        }
    }

    state.remove(id);
    Ok(())
}

/// Write one newline-delimited JSON response frame.
async fn send_line(writer: &mut OwnedWriteHalf, message: &Response) -> Result<()> {
    let mut line = serde_json::to_string(message).context("serializing response")?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}
