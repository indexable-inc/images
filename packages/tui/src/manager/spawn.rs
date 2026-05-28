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

    // Kitty keyboard protocol enhancement so callers can deliver Ctrl+Enter and
    // similar chords without ambiguity. See https://sw.kovidgoyal.net/kitty/keyboard-protocol/
    // Flags: 1 = DISAMBIGUATE_ESCAPE_CODES, 2 = REPORT_EVENT_TYPES, combined = 3.
    let keyboard_enhancement_sequence = b"\x1b[=3u";

    runtime_clone.spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut pty_clone = pty;
        if let Err(e) = pty_clone.write_all(keyboard_enhancement_sequence).await {
            eprintln!("Warning: Failed to enable keyboard enhancements: {e}");
        }
        pty_actor(id, pty_clone, command_rx, parser).await;
    });

    let instance = TuiInstance {
        id,
        command,
        args,
        spawned_at: SystemTime::now(),
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        scrollback_limit: scrollback_lines,
        _process: Arc::new(tokio::sync::Mutex::new(child)),
        command_tx,
    };

    wait_for_initial_output(runtime, &vt100_parser)?;

    Ok(instance)
}
