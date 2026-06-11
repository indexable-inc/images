use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

use crate::actor::engine;
use crate::actor::{PtyCommand, pty_actor};
use crate::manager::TuiInstance;
use crate::manager::reader;
use crate::types::{CursorShape, ExitState, SpawnConfig};
use crate::{Error, Result};

const CHANNEL_BUFFER_SIZE: usize = 100;
const INITIAL_OUTPUT_TIMEOUT: Duration = Duration::from_millis(100);
const INITIAL_OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Give the child a brief window to paint its first frame so callers that read
/// immediately after spawn see content instead of an empty screen.
///
/// Polls the viewport through the actor mailbox rather than the engine directly:
/// `read_viewport` drops trailing blank rows, so a non-empty result means the
/// child has painted something.
fn wait_for_initial_output(runtime: &Runtime, id: Uuid, command_tx: &mpsc::Sender<PtyCommand>) {
    let start = Instant::now();
    runtime.block_on(async {
        loop {
            let painted = reader::read_viewport(id, command_tx)
                .await
                .is_ok_and(|lines| !lines.is_empty());
            if painted || start.elapsed() >= INITIAL_OUTPUT_TIMEOUT {
                break;
            }
            tokio::time::sleep(INITIAL_OUTPUT_POLL_INTERVAL).await;
        }
    });
}

fn process_spawn_error(
    command: &str,
    error: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> Error {
    Error::ProcessSpawn {
        command: command.to_string(),
        source: std::io::Error::other(error),
    }
}

pub(super) fn spawn_tui(
    runtime: &Arc<Runtime>,
    command: String,
    args: Vec<String>,
    config: SpawnConfig,
) -> Result<TuiInstance> {
    let id = Uuid::new_v4();
    let SpawnConfig {
        rows,
        cols,
        scrollback_lines,
    } = config;
    let display = format!("{command} {}", args.join(" "));

    let (pty, child) = runtime.block_on(async {
        // `pty_process::open` allocates the PTY master and its slave together.
        let (pty, pty_slave) = pty_process::open().map_err(|e| process_spawn_error(&display, e))?;

        pty.resize(pty_process::Size::new(rows, cols))
            .map_err(|e| process_spawn_error("resize PTY", e))?;

        // A PTY-backed emulator is only as useful as the TERM the child thinks
        // it is driving. With no TERM the child inherits the host's, so curses
        // and terminfo capabilities (e.g. `curs_set`) silently fail or differ
        // by machine. ix-vt implements an xterm-256color superset, so advertise
        // that plus truecolor for a consistent, capable default.
        let child = pty_process::Command::new(&command)
            .args(&args)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .spawn(pty_slave)
            .map_err(|e| process_spawn_error(&display, e))?;

        Ok::<_, Error>((pty, child))
    })?;

    let (command_tx, command_rx) = mpsc::channel::<PtyCommand>(CHANNEL_BUFFER_SIZE);
    let (exit_tx, exit_rx) = watch::channel(ExitState::Running);
    // The VT engine owns the !Send terminal on its own thread and updates the
    // cursor shape on every render. The cache is read by the frame builder, so
    // it is shared like `size` rather than channelled.
    let cursor_shape = Arc::new(parking_lot::RwLock::new(CursorShape::default()));
    // Whether the child has enabled DECCKM (application cursor keys). The engine
    // refreshes it as it processes output; the actor reads it to pick the arrow
    // form on write. Shared like `cursor_shape` rather than channelled.
    let app_cursor_keys = Arc::new(parking_lot::RwLock::new(false));
    let engine_tx = engine::spawn(
        id,
        rows,
        cols,
        scrollback_lines,
        Arc::clone(&cursor_shape),
        Arc::clone(&app_cursor_keys),
    )?;

    // The actor owns the child: it reaps it (so short-lived commands leave no
    // zombie), publishes the exit code through `exit_tx`, and can signal it on
    // a kill request. It forwards bytes and reads to the engine thread.
    runtime.spawn(async move {
        pty_actor(
            id,
            pty,
            child,
            command_rx,
            engine_tx,
            exit_tx,
            app_cursor_keys,
        )
        .await;
    });

    let instance = TuiInstance {
        id,
        command,
        args,
        spawned_at: SystemTime::now(),
        scrollback_limit: scrollback_lines,
        size: Arc::new(parking_lot::RwLock::new((rows, cols))),
        cursor_shape,
        command_tx,
        exit_rx,
        runtime: Arc::clone(runtime),
    };

    wait_for_initial_output(runtime, id, &instance.command_tx);

    Ok(instance)
}
