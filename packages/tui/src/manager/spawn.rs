use std::sync::Arc;
use std::time::SystemTime;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    actor::{PtyCommand, pty_actor},
    error::Result,
    types::TuiInstance,
};

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const CHANNEL_BUFFER_SIZE: usize = 100;
const INITIAL_OUTPUT_TIMEOUT_MS: u64 = 100;
const INITIAL_OUTPUT_POLL_INTERVAL_MS: u64 = 5;

async fn has_output(parser: &Arc<tokio::sync::RwLock<vt100::Parser>>) -> bool {
    let parser_guard = parser.read().await;
    let screen = parser_guard.screen();
    let contents = screen.contents();
    !contents.is_empty()
}

fn wait_for_initial_output(
    runtime: &Arc<Runtime>,
    parser: &Arc<tokio::sync::RwLock<vt100::Parser>>,
) -> Result<()> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(INITIAL_OUTPUT_TIMEOUT_MS);
    let poll_interval = std::time::Duration::from_millis(INITIAL_OUTPUT_POLL_INTERVAL_MS);

    runtime.block_on(async {
        while !has_output(parser).await && start.elapsed() < timeout {
            tokio::time::sleep(poll_interval).await;
        }
        Ok(())
    })
}

pub(super) fn spawn_tui(
    runtime: &Arc<Runtime>,
    command: String,
    args: Vec<String>,
    scrollback_lines: usize,
) -> Result<TuiInstance> {
    let id = Uuid::new_v4();

    let (pty, child) = runtime.block_on(async {
        let pty = pty_process::Pty::new().map_err(|e| crate::Error::ProcessSpawn {
            command: format!("{command} {}", args.join(" ")),
            source: std::io::Error::other(e),
        })?;

        let pty_slave = pty.pts().map_err(|e| crate::Error::ProcessSpawn {
            command: "get PTY slave".to_string(),
            source: std::io::Error::other(e),
        })?;

        pty.resize(pty_process::Size::new(DEFAULT_ROWS, DEFAULT_COLS))
            .map_err(|e| crate::Error::ProcessSpawn {
                command: "resize PTY".to_string(),
                source: std::io::Error::other(e),
            })?;

        let mut cmd = pty_process::Command::new(&command);
        cmd.args(&args);

        let child = cmd
            .spawn(&pty_slave)
            .map_err(|e| crate::Error::ProcessSpawn {
                command: format!("{command} {}", args.join(" ")),
                source: std::io::Error::other(e),
            })?;

        Ok::<_, crate::Error>((pty, child))
    })?;

    let vt100_parser = Arc::new(tokio::sync::RwLock::new(vt100::Parser::new(
        DEFAULT_ROWS,
        DEFAULT_COLS,
        scrollback_lines,
    )));

    let (command_tx, command_rx) = mpsc::channel::<PtyCommand>(CHANNEL_BUFFER_SIZE);

    let parser = Arc::clone(&vt100_parser);
    let runtime_clone = Arc::clone(runtime);

    runtime_clone.spawn(async move {
        pty_actor(id, pty, command_rx, parser).await;
    });

    // Own the child entirely in a reaper task. Calling wait() drives the
    // SIGCHLD reap so short-lived commands (echo, seq, ...) do not leave
    // zombies even though we never expose a kill path on TuiInstance.
    runtime_clone.spawn(async move {
        let mut child = child;
        let _ = child.wait().await;
    });

    let instance = TuiInstance {
        id,
        command,
        args,
        spawned_at: SystemTime::now(),
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        scrollback_limit: scrollback_lines,
        command_tx,
    };

    wait_for_initial_output(runtime, &vt100_parser)?;

    Ok(instance)
}
