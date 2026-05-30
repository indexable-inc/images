mod reader;
mod spawn;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ndarray::Array2;
use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::actor::PtyCommand;
use crate::types::{CursorShape, ExitState, FullOutput, SpawnConfig, StyledCell};
use crate::{Error, Result};

/// A handle to one spawned PTY-backed process.
///
/// The handle owns everything it needs to talk to its process: the actor
/// channel and a clone of the manager's runtime. Blocking methods drive that
/// runtime to completion; each has an `_async` twin that returns a future
/// instead. Cloning a handle is cheap and every clone addresses the same
/// process.
#[derive(Clone)]
pub struct TuiInstance {
    /// Stable identifier assigned at spawn.
    pub id: Uuid,
    /// The command that was spawned.
    pub command: String,
    /// Positional arguments passed to the command.
    pub args: Vec<String>,
    /// When the process was spawned.
    pub spawned_at: SystemTime,
    /// Configured scrollback depth.
    pub scrollback_limit: usize,
    /// Live terminal size, shared across clones so a [`resize`](Self::resize)
    /// on one handle is visible from every handle to the same process.
    pub(crate) size: Arc<RwLock<(u16, u16)>>,
    /// Latest cursor shape, updated by the VT engine on every render. Shared
    /// across clones like `size` so the frame builder can read it synchronously.
    pub(crate) cursor_shape: Arc<RwLock<CursorShape>>,
    pub(crate) command_tx: mpsc::Sender<PtyCommand>,
    pub(crate) exit_rx: watch::Receiver<ExitState>,
    pub(crate) runtime: Arc<Runtime>,
}

impl TuiInstance {
    /// Current terminal height in rows.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.size.read().0
    }

    /// Current terminal width in columns.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.size.read().1
    }

    /// The cursor shape the child last requested via `DECSCUSR`, or the default
    /// block. Reads cached state, so it stays synchronous.
    #[must_use]
    pub fn cursor_shape(&self) -> CursorShape {
        *self.cursor_shape.read()
    }

    /// The cursor's `(row, col, visible)` in viewport cell coordinates.
    pub fn read_cursor(&self) -> Result<(u16, u16, bool)> {
        self.runtime
            .block_on(reader::read_cursor(self.id, &self.command_tx))
    }

    /// [`TuiInstance::read_cursor`] as a future.
    pub async fn read_cursor_async(&self) -> Result<(u16, u16, bool)> {
        reader::read_cursor(self.id, &self.command_tx).await
    }

    /// Resize the terminal to `rows` x `cols`.
    ///
    /// Resizes the kernel PTY window (which delivers `SIGWINCH` to the child)
    /// and the VT100 emulator together, so subsequent reads see the new
    /// geometry. Visible from every handle to this process.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.runtime
            .block_on(reader::resize(self.id, &self.command_tx, rows, cols))?;
        *self.size.write() = (rows, cols);
        Ok(())
    }

    /// [`TuiInstance::resize`] as a future.
    pub async fn resize_async(&self, rows: u16, cols: u16) -> Result<()> {
        reader::resize(self.id, &self.command_tx, rows, cols).await?;
        *self.size.write() = (rows, cols);
        Ok(())
    }

    /// Send `data` to the PTY exactly as given.
    pub fn write(&self, data: &str) -> Result<()> {
        self.runtime.block_on(reader::write(
            self.id,
            &self.command_tx,
            data.as_bytes().to_vec(),
        ))
    }

    /// The current viewport as one string per visible row.
    pub fn read_viewport(&self) -> Result<Vec<String>> {
        self.runtime
            .block_on(reader::read_viewport(self.id, &self.command_tx))
    }

    /// Lines that have scrolled above the viewport, oldest first.
    pub fn read_scrollback(&self) -> Result<Vec<String>> {
        self.runtime
            .block_on(reader::read_scrollback(self.id, &self.command_tx))
    }

    /// Scrollback and viewport read together.
    pub fn read_full(&self) -> Result<FullOutput> {
        self.runtime
            .block_on(reader::read_full(self.id, &self.command_tx))
    }

    /// Read the viewport, retrying until output appears or `timeout` elapses.
    pub fn read_blocking(&self, timeout: Duration) -> Result<Vec<String>> {
        self.runtime
            .block_on(reader::read_blocking(self.id, &self.command_tx, timeout))
    }

    /// Viewport characters as a `rows x cols` grid.
    pub fn read_chars(&self) -> Result<Vec<Vec<char>>> {
        self.runtime
            .block_on(reader::read_chars(self.id, &self.command_tx))
    }

    /// Per-cell character and styling for the whole viewport.
    pub fn read_styled_cells(&self) -> Result<Array2<StyledCell>> {
        self.runtime
            .block_on(reader::read_styled_cells(self.id, &self.command_tx))
    }

    // -- lifecycle --------------------------------------------------------

    /// The current lifecycle state: running, or exited with a code.
    #[must_use]
    pub fn exit_state(&self) -> ExitState {
        *self.exit_rx.borrow()
    }

    /// Whether the child process is still running.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        matches!(*self.exit_rx.borrow(), ExitState::Running)
    }

    /// Block until the child exits, or until `timeout` elapses if given.
    ///
    /// Returns the exit state once it has exited, or `None` on timeout. A
    /// dropped actor (its sender gone) counts as exited.
    #[must_use]
    pub fn wait(&self, timeout: Option<Duration>) -> Option<ExitState> {
        let mut rx = self.exit_rx.clone();
        self.runtime.block_on(async move {
            let settle = rx.wait_for(|state| matches!(state, ExitState::Exited(_)));
            match timeout {
                Some(timeout) => match tokio::time::timeout(timeout, settle).await {
                    // Resolved (exited) or the sender dropped: either way, done.
                    Ok(Ok(state)) => Some(*state),
                    Ok(Err(_)) => Some(ExitState::Exited(None)),
                    Err(_) => None,
                },
                None => Some(settle.await.map_or(ExitState::Exited(None), |state| *state)),
            }
        })
    }

    /// Force-terminate the child with `SIGKILL`.
    ///
    /// A no-op if the child already exited. Unlike a cooperative Ctrl+C this
    /// cannot be ignored, so it is the reliable way to stop a program that
    /// traps interrupts (an editor in normal mode, a stuck REPL).
    pub fn kill(&self) -> Result<()> {
        self.runtime
            .block_on(reader::kill(self.id, &self.command_tx))
    }

    /// [`TuiInstance::write`] as a future.
    pub async fn write_async(&self, data: &str) -> Result<()> {
        reader::write(self.id, &self.command_tx, data.as_bytes().to_vec()).await
    }

    /// [`TuiInstance::read_viewport`] as a future.
    pub async fn read_viewport_async(&self) -> Result<Vec<String>> {
        reader::read_viewport(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::read_scrollback`] as a future.
    pub async fn read_scrollback_async(&self) -> Result<Vec<String>> {
        reader::read_scrollback(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::read_full`] as a future.
    pub async fn read_full_async(&self) -> Result<FullOutput> {
        reader::read_full(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::read_blocking`] as a future.
    pub async fn read_blocking_async(&self, timeout: Duration) -> Result<Vec<String>> {
        reader::read_blocking(self.id, &self.command_tx, timeout).await
    }

    /// [`TuiInstance::read_chars`] as a future.
    pub async fn read_chars_async(&self) -> Result<Vec<Vec<char>>> {
        reader::read_chars(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::read_styled_cells`] as a future.
    pub async fn read_styled_cells_async(&self) -> Result<Array2<StyledCell>> {
        reader::read_styled_cells(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::kill`] as a future.
    pub async fn kill_async(&self) -> Result<()> {
        reader::kill(self.id, &self.command_tx).await
    }

    /// [`TuiInstance::wait`] as a future, without the timeout branch.
    pub async fn wait_async(&self) -> ExitState {
        let mut rx = self.exit_rx.clone();
        rx.wait_for(|state| matches!(state, ExitState::Exited(_)))
            .await
            .map_or(ExitState::Exited(None), |state| *state)
    }
}

/// Spawns PTY-backed processes and tracks the live ones.
///
/// The manager owns the tokio runtime that drives every spawned actor and
/// shares a clone of it into each [`TuiInstance`], so a handle keeps working
/// for as long as it is held.
pub struct TuiManager {
    instances: Arc<RwLock<HashMap<Uuid, TuiInstance>>>,
    runtime: Arc<Runtime>,
}

impl Default for TuiManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiManager {
    /// Create a manager with a fresh multi-threaded tokio runtime.
    ///
    /// # Panics
    /// Panics if the tokio runtime cannot be created.
    #[must_use]
    pub fn new() -> Self {
        let runtime = Runtime::new()
            .unwrap_or_else(|e| panic!("failed to create tokio runtime for TUI manager: {e}"));

        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            runtime: Arc::new(runtime),
        }
    }

    /// Spawn `command` with `args` on a fresh PTY sized per `config`.
    pub fn spawn(
        &self,
        command: String,
        args: Vec<String>,
        config: SpawnConfig,
    ) -> Result<TuiInstance> {
        let instance = spawn::spawn_tui(&self.runtime, command, args, config)?;
        self.instances.write().insert(instance.id, instance.clone());
        Ok(instance)
    }

    /// Every instance the manager currently tracks.
    #[must_use]
    pub fn list(&self) -> Vec<TuiInstance> {
        self.instances.read().values().cloned().collect()
    }

    /// Stop tracking the instance with `id`, returning it if it was present.
    ///
    /// The actor keeps running until every handle is dropped, so a caller that
    /// wants the process gone should [`TuiInstance::kill`] it first. Removal is
    /// what drops an exited terminal out of [`list`](Self::list) and the
    /// dashboard.
    #[must_use]
    pub fn remove(&self, id: &Uuid) -> Option<TuiInstance> {
        self.instances.write().remove(id)
    }

    /// Look up a tracked instance by id.
    pub fn get(&self, id: &Uuid) -> Result<TuiInstance> {
        self.instances
            .read()
            .get(id)
            .cloned()
            .ok_or(Error::TuiNotFound { id: *id })
    }

    /// A handle to the manager's long-lived runtime.
    ///
    /// The dashboard and producer spawn their server, poll, and accept loops on
    /// this runtime rather than the ambient one, so a sync caller's
    /// `Runtime::new().block_on(serve(..))` returns a dashboard that keeps
    /// running after that temporary runtime is dropped.
    #[cfg(any(feature = "dashboard", feature = "publish"))]
    pub(crate) fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.runtime.handle().clone()
    }
}
