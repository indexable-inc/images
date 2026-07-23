//! Headless NDJSON emitter: the machine-readable counterpart to the web UI.
//!
//! The web server streams msgpack deltas to a browser. A programmatic consumer
//! (the kernel `nix` module's live-pane path) wants the same parser as the
//! single owner of internal-json, but as plain lines on stdout, not a socket.
//! [`run`] spawns the nix command exactly as the server does
//! (`--log-format internal-json`), feeds each stderr line through
//! [`MonitorState`], and prints the compact [`BuildView`](nix_web_monitor_parser::BuildView)
//! as one JSON object per line, throttled so a chatty build cannot flood the
//! consumer. Our stdout is reserved for that NDJSON, so nix's own stdout (its
//! command output) is redirected to our stderr rather than mixed in, and the
//! emitter exits with nix's status.
//!
//! No HTTP server, no broadcast channel, no daemon probe: the emitter is the
//! thin, dependency-light path, and the render model it emits is owned by the
//! parser crate so the dashboard renderer and this emitter never drift.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use nix_web_monitor_parser::MonitorState;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::{Instant, MissedTickBehavior, interval};

/// Minimum wall-clock between emitted snapshots. Nix emits a line per activity
/// event; at ~2 Hz the consumer sees prompt progress without a line per event
/// (which would defeat the point of a compact projection and hammer the
/// dashboard's wholesale-diffed pane). A change that lands between ticks rides
/// the next tick; the final snapshot after exit is always emitted.
const EMIT_INTERVAL: Duration = Duration::from_millis(500);

/// Run `nix <args>` under the parser and emit throttled `BuildView` NDJSON on
/// stdout, returning nix's exit code (`None` if it reported none).
///
/// `nix_verbose` passes `-v` to nix for richer activity events, matching the
/// server's `--nix-verbose`.
pub async fn run(nix_args: Vec<String>, nix_verbose: bool) -> Result<Option<i32>> {
    let command_label = format!("nix {}", nix_args.join(" "));
    let mut state = MonitorState::new(command_label);

    let mut command = Command::new("nix");
    if nix_verbose {
        command.arg("-v");
    }
    command
        .arg("--log-format")
        .arg("internal-json")
        .args(&nix_args)
        // stdout is nix's real output (e.g. `nix eval --raw`); forward it to our
        // own stdout is wrong here because we emit NDJSON there, so drain it to
        // stderr instead, keeping stdout a clean NDJSON channel. The events we
        // parse ride stderr.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("spawning nix")?;
    let child_stdout = child.stdout.take().context("nix stdout was not captured")?;
    let child_stderr = child.stderr.take().context("nix stderr was not captured")?;

    // Forward nix's stdout to our stderr so it is visible for debugging without
    // corrupting the NDJSON channel on our stdout.
    let stdout_task = tokio::spawn(async move {
        let mut reader = child_stdout;
        let _ = io::copy(&mut reader, &mut io::stderr()).await;
    });

    let mut lines = BufReader::new(child_stderr);
    let mut buf: Vec<u8> = Vec::new();
    let mut ticker = interval(EMIT_INTERVAL);
    // The first tick fires immediately; skip a missed tick rather than burst to
    // catch up, so a slow consumer never gets a backlog of snapshots at once.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    // Whether state changed since the last emit, so an idle interval (a long
    // silent build phase) does not re-emit an identical snapshot every tick.
    let mut dirty = false;
    // The last tick we actually emitted on, so a burst of events between ticks
    // still emits at most once per interval.
    let mut last_emit = Instant::now();

    loop {
        buf.clear();
        tokio::select! {
            read = lines.read_until(b'\n', &mut buf) => {
                let read = read.context("reading nix stderr")?;
                if read == 0 {
                    break;
                }
                trim_eol(&mut buf);
                // Lossy decode so a builder's non-UTF-8 byte never stalls the
                // pipe (and then blocks the child on a full stderr), matching
                // the server's stderr reader.
                let line = String::from_utf8_lossy(&buf);
                state.apply_line(&line);
                dirty = true;
                // Coalesce: emit at most once per interval even under a burst.
                if last_emit.elapsed() >= EMIT_INTERVAL {
                    emit(&state).await?;
                    dirty = false;
                    last_emit = Instant::now();
                }
            }
            _ = ticker.tick() => {
                if dirty {
                    emit(&state).await?;
                    dirty = false;
                    last_emit = Instant::now();
                }
            }
        }
    }

    let status = child.wait().await.context("waiting for nix")?;
    let exit_code = status.code();
    // Settle still-running/stopped builds against the real exit code (Nix never
    // marks a build succeeded; a clean exit promotes them), then emit the final
    // snapshot so the consumer's last line is authoritative.
    state.finish(exit_code);
    emit(&state).await?;

    // The stdout drain ends when the child's stdout closes; join so a late line
    // is flushed before we exit.
    let _ = stdout_task.await;
    Ok(exit_code)
}

/// Serialize the current build view and write it as one NDJSON line on stdout,
/// flushing so the consumer sees it promptly (a buffered pipe would otherwise
/// hold it until the buffer filled or the process exited).
async fn emit(state: &MonitorState) -> Result<()> {
    let mut line = serde_json::to_vec(&state.build_view()).context("serializing build view")?;
    line.push(b'\n');
    let mut stdout = io::stdout();
    stdout.write_all(&line).await.context("writing NDJSON")?;
    stdout.flush().await.context("flushing NDJSON")?;
    Ok(())
}

/// Drop a trailing `\n` and any preceding `\r` from a read line.
fn trim_eol(buf: &mut Vec<u8>) {
    if buf.last() == Some(&b'\n') {
        buf.pop();
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
}
