//! Session lifecycle: server-spawned PTYs, one VT engine per session, and
//! the `/run/ix-term` contract the `ixterm` CLI resolves against.
//!
//! A session is a login shell on a PTY spawned on the serving host. The
//! server is the single source of truth for terminal state: PTY output runs
//! through the OSC 5522 scanner, the cleaned bytes feed the libghostty-vt
//! engine, and every websocket client renders the engine's dirty-row frames.
//!
//! The CLI contract (`packages/ixterm/src/pts.rs`): the child environment
//! carries `IX_TERM_SESSION_ID=<id>`, and `<runtime dir>/sessions/<id>/pts`
//! contains the session's pts path, so `ixterm open` can write the private
//! OSC straight to the slave side.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

use crate::osc::{OscEvent, Scanner};
use crate::proto::{ServerMsg, SessionMetaWire};
use crate::vt::{EngineHandle, EngineMsg, spawn_engine};

/// Initial PTY size before any driver resizes it.
const DEFAULT_ROWS: u16 = 24;
/// Initial PTY width before any driver resizes it.
const DEFAULT_COLS: u16 = 80;

/// Broadcast ring per session. A client that falls this far behind is
/// resynced with a full frame instead of the dropped ones.
const EVENTS_CHANNEL_CAPACITY: usize = 256;

/// Server-wide session configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Base runtime directory (`/run/ix-term` in production); pts mappings
    /// live under `<runtime_dir>/sessions/<id>/pts`.
    pub runtime_dir: PathBuf,
    /// Shell to spawn; defaults to `$SHELL` then `/bin/sh`.
    pub shell: Option<String>,
    /// Scrollback lines kept by the VT engine.
    pub scrollback: usize,
}

/// The driver seat: who owns the PTY size.
///
/// The seat follows the last keystroke (index#3797's "active driver owns the
/// PTY size"); a resize is honored only from the holder, or claims the seat
/// when it is free, so viewers never fight over dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverSeat {
    conn: Option<u64>,
    /// Authoritative grid height.
    pub rows: u16,
    /// Authoritative grid width.
    pub cols: u16,
}

impl DriverSeat {
    const fn new(rows: u16, cols: u16) -> Self {
        Self {
            conn: None,
            rows,
            cols,
        }
    }

    /// Take the seat; returns whether the holder changed.
    fn claim(&mut self, conn: u64) -> bool {
        let changed = self.conn != Some(conn);
        self.conn = Some(conn);
        changed
    }

    /// Free the seat if `conn` holds it; returns whether it was freed.
    fn release(&mut self, conn: u64) -> bool {
        if self.conn == Some(conn) {
            self.conn = None;
            true
        } else {
            false
        }
    }

    /// Whether `conn` may resize: it holds the seat or the seat is free.
    const fn may_resize(&self, conn: u64) -> bool {
        match self.conn {
            Some(holder) => holder == conn,
            None => true,
        }
    }
}

/// State of the session's opened document (the OSC 5522 split view).
#[derive(Debug, Default)]
struct DocState {
    path: Option<String>,
    nonce: u64,
}

/// A live terminal session.
pub struct Session {
    /// The session id (also the runtime directory name).
    pub id: Uuid,
    name: std::sync::Mutex<String>,
    created_at_ms: u64,
    engine: EngineHandle,
    events: broadcast::Sender<Arc<ServerMsg>>,
    writer: tokio::sync::Mutex<pty_process::OwnedWritePty>,
    seat: std::sync::Mutex<DriverSeat>,
    doc: std::sync::Mutex<DocState>,
    next_conn: AtomicU64,
    /// Flipping to `true` asks the PTY task to kill the child.
    kill: watch::Sender<bool>,
}

impl Session {
    /// The session's wire metadata.
    pub fn meta(&self) -> SessionMetaWire {
        SessionMetaWire {
            id: self.id.to_string(),
            name: self.name.lock().expect("name lock").clone(),
            created_at_ms: self.created_at_ms,
        }
    }

    /// Allocate a connection id for a new websocket client.
    pub fn next_conn(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    /// Subscribe to the session's event stream (grid frames, driver changes,
    /// opens, exit).
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<ServerMsg>> {
        self.events.subscribe()
    }

    /// Broadcast `msg` to every connected client.
    pub fn broadcast(&self, msg: ServerMsg) {
        // No receivers just means no viewers right now.
        let _ = self.events.send(Arc::new(msg));
    }

    /// Ask the engine for a full frame (new client or lag resync).
    pub fn request_full_frame(&self) {
        self.engine.send(EngineMsg::FullFrame);
    }

    /// The current driver-seat state as a wire message.
    pub fn driver_msg(&self) -> ServerMsg {
        let seat = *self.seat.lock().expect("seat lock");
        ServerMsg::Driver {
            conn: seat.conn.map(|c| c.to_string()),
            cols: seat.cols,
            rows: seat.rows,
        }
    }

    /// The current opened-document state as a wire message.
    pub fn open_msg(&self) -> ServerMsg {
        let doc = self.doc.lock().expect("doc lock");
        ServerMsg::Open {
            path: doc.path.clone(),
            nonce: doc.nonce,
        }
    }

    /// The absolute path of the currently opened document, if any.
    pub fn doc_path(&self) -> Option<String> {
        self.doc.lock().expect("doc lock").path.clone()
    }

    /// Write client input to the PTY; the sender takes the driver seat.
    pub async fn write_input(&self, conn: u64, data: &str) {
        let claimed = self.seat.lock().expect("seat lock").claim(conn);
        if claimed {
            self.broadcast(self.driver_msg());
        }
        let mut writer = self.writer.lock().await;
        if let Err(error) = writer.write_all(data.as_bytes()).await {
            tracing::warn!(%error, session = %self.id, "PTY write failed");
        }
    }

    /// Resize the PTY if `conn` may (driver, or free seat which it claims).
    pub async fn resize(&self, conn: u64, rows: u16, cols: u16) {
        {
            let mut seat = self.seat.lock().expect("seat lock");
            if !seat.may_resize(conn) {
                return;
            }
            seat.claim(conn);
            seat.rows = rows;
            seat.cols = cols;
        }
        {
            let writer = self.writer.lock().await;
            if let Err(error) = writer.resize(pty_process::Size::new(rows, cols)) {
                tracing::warn!(%error, session = %self.id, "PTY resize failed");
            }
        }
        self.engine.send(EngineMsg::Resize { rows, cols });
        self.broadcast(self.driver_msg());
    }

    /// Release the driver seat when a client disconnects.
    pub fn release_driver(&self, conn: u64) {
        let released = self.seat.lock().expect("seat lock").release(conn);
        if released {
            self.broadcast(self.driver_msg());
        }
    }

    /// Close the opened document for every viewer.
    pub fn close_doc(&self) {
        {
            let mut doc = self.doc.lock().expect("doc lock");
            doc.path = None;
        }
        self.broadcast(self.open_msg());
    }

    /// Apply one parsed OSC 5522 event from the PTY stream.
    async fn handle_osc(&self, event: OscEvent) {
        match event {
            OscEvent::Open(path) => match tokio::fs::metadata(&path).await {
                Ok(meta) if meta.is_file() => {
                    {
                        let mut doc = self.doc.lock().expect("doc lock");
                        doc.path = Some(path);
                        doc.nonce += 1;
                    }
                    self.broadcast(self.open_msg());
                }
                Ok(_) => self.broadcast(ServerMsg::OpenError {
                    message: format!("open: {path} is not a regular file"),
                }),
                Err(error) => self.broadcast(ServerMsg::OpenError {
                    message: format!("open: cannot read {path}: {error}"),
                }),
            },
            OscEvent::Malformed(message) => self.broadcast(ServerMsg::OpenError { message }),
        }
    }

    fn request_kill(&self) {
        // Send fails only when the PTY task already exited, which is the goal.
        let _ = self.kill.send(true);
    }
}

/// All live sessions plus the watch feed for the tab bar.
pub struct SessionManager {
    config: ServerConfig,
    sessions: tokio::sync::RwLock<HashMap<Uuid, Arc<Session>>>,
    list_tx: watch::Sender<Vec<SessionMetaWire>>,
}

impl SessionManager {
    /// Create a manager and its runtime directory.
    ///
    /// # Errors
    /// Fails if the runtime sessions directory cannot be created (fail fast:
    /// without it the `ixterm` CLI contract cannot be honored).
    pub fn new(config: ServerConfig) -> anyhow::Result<Self> {
        let sessions_dir = config.runtime_dir.join("sessions");
        std::fs::create_dir_all(&sessions_dir)
            .with_context(|| format!("cannot create {}", sessions_dir.display()))?;
        let (list_tx, _) = watch::channel(Vec::new());
        Ok(Self {
            config,
            sessions: tokio::sync::RwLock::new(HashMap::new()),
            list_tx,
        })
    }

    /// Watch the live session list (for the `/api/ws` events socket).
    pub fn watch_list(&self) -> watch::Receiver<Vec<SessionMetaWire>> {
        self.list_tx.subscribe()
    }

    /// The current session list, oldest first.
    pub async fn list(&self) -> Vec<SessionMetaWire> {
        let mut list: Vec<_> = self
            .sessions
            .read()
            .await
            .values()
            .map(|s| (s.created_at_ms, s.meta()))
            .collect();
        list.sort_by_key(|(created, meta)| (*created, meta.id.clone()));
        list.into_iter().map(|(_, meta)| meta).collect()
    }

    /// Look up a session.
    pub async fn get(&self, id: Uuid) -> Option<Arc<Session>> {
        self.sessions.read().await.get(&id).cloned()
    }

    /// Spawn a new session: PTY + login shell + VT engine + runtime files.
    ///
    /// # Errors
    /// Fails if the PTY cannot be opened, the shell cannot be spawned, the
    /// engine thread cannot start, or the runtime files cannot be written.
    pub async fn create(&self, name: Option<String>) -> anyhow::Result<Arc<Session>> {
        let id = Uuid::new_v4();
        let name = name.unwrap_or_else(|| "shell".to_owned());

        let (pty, pts) = pty_process::open().context("cannot open PTY")?;
        pty.resize(pty_process::Size::new(DEFAULT_ROWS, DEFAULT_COLS))
            .context("cannot size PTY")?;
        let pts_path = pts_path_of(&pty)?;

        let shell = self
            .config
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".to_owned());
        // `-l` makes it a login shell: sessions are the user's real
        // environment, matching what they would get on the host.
        let child = pty_process::Command::new(&shell)
            .arg("-l")
            .env("IX_TERM_SESSION_ID", id.to_string())
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .spawn(pts)
            .with_context(|| format!("cannot spawn {shell}"))?;

        // Publish the pts mapping the `ixterm` CLI resolves
        // (packages/ixterm/src/pts.rs trims and requires an absolute path).
        let dir = self.session_dir(id);
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("cannot create {}", dir.display()))?;
        tokio::fs::write(dir.join("pts"), format!("{pts_path}\n"))
            .await
            .with_context(|| format!("cannot write {}", dir.join("pts").display()))?;

        let (events, _) = broadcast::channel(EVENTS_CHANNEL_CAPACITY);
        let engine = spawn_engine(
            DEFAULT_ROWS,
            DEFAULT_COLS,
            self.config.scrollback,
            events.clone(),
        )?;

        let (read_half, write_half) = pty.into_split();
        let (kill_tx, kill_rx) = watch::channel(false);
        let session = Arc::new(Session {
            id,
            name: std::sync::Mutex::new(name),
            created_at_ms: unix_millis(),
            engine,
            events,
            writer: tokio::sync::Mutex::new(write_half),
            seat: std::sync::Mutex::new(DriverSeat::new(DEFAULT_ROWS, DEFAULT_COLS)),
            doc: std::sync::Mutex::new(DocState::default()),
            next_conn: AtomicU64::new(1),
            kill: kill_tx,
        });

        tokio::spawn(pty_task(Arc::clone(&session), read_half, child, kill_rx));

        self.sessions.write().await.insert(id, Arc::clone(&session));
        self.publish_list().await;
        Ok(session)
    }

    /// Rename a session; returns whether it existed.
    pub async fn rename(&self, id: Uuid, name: String) -> bool {
        let renamed = self.sessions.read().await.get(&id).is_some_and(|session| {
            *session.name.lock().expect("name lock") = name;
            true
        });
        if renamed {
            self.publish_list().await;
        }
        renamed
    }

    /// Kill and remove a session; returns whether it existed.
    pub async fn close(&self, id: Uuid) -> bool {
        let Some(session) = self.sessions.write().await.remove(&id) else {
            return false;
        };
        session.request_kill();
        let dir = self.session_dir(id);
        if let Err(error) = tokio::fs::remove_dir_all(&dir).await {
            tracing::warn!(%error, dir = %dir.display(), "cannot remove session runtime dir");
        }
        self.publish_list().await;
        true
    }

    fn session_dir(&self, id: Uuid) -> PathBuf {
        self.config
            .runtime_dir
            .join("sessions")
            .join(id.to_string())
    }

    async fn publish_list(&self) {
        self.list_tx.send_replace(self.list().await);
    }
}

/// Milliseconds since the Unix epoch.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() * 1000 + u64::from(d.subsec_millis()))
}

/// The slave pts path of a PTY master, via `ptsname(3)`.
fn pts_path_of(pty: &pty_process::Pty) -> anyhow::Result<String> {
    let fd = pty.as_raw_fd();
    #[cfg(target_os = "linux")]
    {
        let mut buf = [0u8; 128];
        // SAFETY: `fd` is a live PTY master owned by `pty`; `buf` outlives
        // the call and its length is passed, so `ptsname_r` cannot overrun.
        let rc = unsafe { libc::ptsname_r(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc)).context("ptsname_r failed");
        }
        let cstr = std::ffi::CStr::from_bytes_until_nul(&buf).context("ptsname_r output")?;
        Ok(cstr.to_str().context("pts path is not UTF-8")?.to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Darwin has no ptsname_r; ptsname returns thread-local storage that
        // is copied out immediately. This path exists for local development
        // only — the packaged server is linux-only.
        // SAFETY: `fd` is a live PTY master; the returned pointer is only
        // read before any other pty call on this thread.
        let ptr = unsafe { libc::ptsname(fd) };
        if ptr.is_null() {
            anyhow::bail!("ptsname failed for fd {fd}");
        }
        // SAFETY: a non-null ptsname result is a valid NUL-terminated string.
        let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
        Ok(cstr.to_str().context("pts path is not UTF-8")?.to_owned())
    }
}

/// The PTY task: owns the read half and the child, feeds the scanner and the
/// engine, reaps the child, and announces its exit.
async fn pty_task(
    session: Arc<Session>,
    mut read_half: pty_process::OwnedReadPty,
    mut child: tokio::process::Child,
    mut kill: watch::Receiver<bool>,
) {
    let mut scanner = Scanner::new();
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            read = read_half.read(&mut buf) => match read {
                // EOF or read error both mean the slave side is gone.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut clean = Vec::with_capacity(n);
                    let mut events = Vec::new();
                    scanner.feed(&buf[..n], &mut clean, &mut events);
                    if !clean.is_empty() {
                        session.engine.send(EngineMsg::Feed(clean));
                    }
                    for event in events {
                        session.handle_osc(event).await;
                    }
                }
            },
            changed = kill.changed() => {
                if changed.is_err() || *kill.borrow() {
                    if let Err(error) = child.start_kill() {
                        tracing::debug!(%error, session = %session.id, "child kill failed");
                    }
                    break;
                }
            }
        }
    }
    let code = child.wait().await.ok().and_then(|status| status.code());
    let mut tail = Vec::new();
    scanner.flush_pending(&mut tail);
    if !tail.is_empty() {
        session.engine.send(EngineMsg::Feed(tail));
    }
    session.broadcast(ServerMsg::Exit { code });
    session.engine.send(EngineMsg::Shutdown);
}

#[cfg(test)]
mod tests {
    use super::{DriverSeat, ServerConfig, SessionManager};
    use crate::proto::ServerMsg;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn seat_follows_last_claim_and_gates_resize() {
        let mut seat = DriverSeat::new(24, 80);
        assert!(seat.may_resize(1), "free seat is resizable by anyone");
        assert!(seat.claim(1));
        assert!(!seat.claim(1), "re-claim by the holder is not a change");
        assert!(seat.may_resize(1));
        assert!(!seat.may_resize(2), "non-driver may not resize");
        assert!(seat.claim(2), "last keystroke wins the seat");
        assert!(!seat.release(1), "stale holder cannot release");
        assert!(seat.release(2));
        assert!(seat.may_resize(1));
    }

    fn test_manager(dir: &std::path::Path) -> SessionManager {
        SessionManager::new(ServerConfig {
            runtime_dir: dir.to_path_buf(),
            shell: Some("/bin/sh".to_owned()),
            scrollback: 100,
        })
        .expect("manager")
    }

    /// Wait until a grid frame's changed rows contain `needle`.
    async fn wait_for_text(
        rx: &mut tokio::sync::broadcast::Receiver<Arc<ServerMsg>>,
        needle: &str,
    ) {
        loop {
            let msg = rx.recv().await.expect("event stream open");
            if let ServerMsg::Grid { changed, .. } = msg.as_ref() {
                let text: String = changed
                    .iter()
                    .flat_map(|row| row.spans.iter().map(|s| s.text.as_str()))
                    .collect();
                if text.contains(needle) {
                    return;
                }
            }
        }
    }

    /// Wait for an event matching a predicate.
    async fn wait_for_msg(
        rx: &mut tokio::sync::broadcast::Receiver<Arc<ServerMsg>>,
        mut pred: impl FnMut(&ServerMsg) -> bool,
    ) {
        loop {
            let msg = rx.recv().await.expect("event stream open");
            if pred(msg.as_ref()) {
                return;
            }
        }
    }

    #[tokio::test]
    async fn session_lifecycle_pts_env_osc_and_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_manager(dir.path());

        let session = manager
            .create(Some("test".to_owned()))
            .await
            .expect("create");
        let id = session.id;
        let mut rx = session.subscribe();

        // The CLI contract: sessions/<id>/pts holds the absolute pts path.
        let pts_file = dir.path().join("sessions").join(id.to_string()).join("pts");
        let pts_path = std::fs::read_to_string(&pts_file).expect("pts file");
        let pts_path = pts_path.trim().to_owned();
        assert!(
            pts_path.starts_with('/'),
            "pts path {pts_path:?} not absolute"
        );

        // The child sees IX_TERM_SESSION_ID: print it and watch the grid.
        tokio::time::timeout(Duration::from_secs(20), async {
            session
                .write_input(1, "printf 'sid=%s\\n' \"$IX_TERM_SESSION_ID\"\r")
                .await;
            wait_for_text(&mut rx, &format!("sid={id}")).await;
        })
        .await
        .expect("session id echoed to the grid");

        // OSC 5522 from the slave side, exactly as `ixterm open` writes it.
        let html = dir.path().join("view.html");
        std::fs::write(&html, "<h1>hi</h1>").expect("write html");
        let osc = format!("\u{1b}]5522;open;{}\u{7}", html.display());
        tokio::time::timeout(Duration::from_secs(20), async {
            std::fs::write(&pts_path, osc.as_bytes()).expect("write OSC to pts");
            wait_for_msg(&mut rx, |msg| {
                matches!(msg, ServerMsg::Open { path: Some(p), .. } if p.ends_with("view.html"))
            })
            .await;
        })
        .await
        .expect("open event broadcast");
        assert_eq!(
            session.doc_path().as_deref(),
            Some(html.to_str().expect("utf8 path"))
        );

        // A missing file renders an error in the session instead of opening.
        tokio::time::timeout(Duration::from_secs(20), async {
            std::fs::write(&pts_path, b"\x1b]5522;open;/nonexistent-ix-term.html\x07")
                .expect("write OSC to pts");
            wait_for_msg(&mut rx, |msg| matches!(msg, ServerMsg::OpenError { .. })).await;
        })
        .await
        .expect("open error broadcast");

        // Close: the child is killed, Exit lands, and the runtime dir is gone.
        tokio::time::timeout(Duration::from_secs(20), async {
            assert!(manager.close(id).await);
            wait_for_msg(&mut rx, |msg| matches!(msg, ServerMsg::Exit { .. })).await;
        })
        .await
        .expect("exit broadcast");
        assert!(!pts_file.exists(), "runtime dir removed on close");
        assert!(manager.get(id).await.is_none());
        assert_eq!(manager.list().await.len(), 0);
    }

    #[tokio::test]
    async fn rename_updates_the_watched_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = test_manager(dir.path());
        let mut list_rx = manager.watch_list();

        let session = manager.create(None).await.expect("create");
        assert_eq!(session.meta().name, "shell");
        list_rx.changed().await.expect("list update");
        assert_eq!(list_rx.borrow_and_update().len(), 1);

        assert!(manager.rename(session.id, "build".to_owned()).await);
        list_rx.changed().await.expect("list update");
        assert_eq!(list_rx.borrow_and_update()[0].name, "build");

        assert!(manager.close(session.id).await);
    }
}
