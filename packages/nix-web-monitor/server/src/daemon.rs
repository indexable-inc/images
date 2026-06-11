//! Live `nix-daemon` syscall tracer (best-effort, privileged).
//!
//! Attaches a platform tracer to the running `nix-daemon` and folds its
//! filesystem syscalls into [`DaemonInfo`], which the UI renders as the daemon
//! panel. This is the one view the internal-json stream cannot give: it stays
//! quiet inside a single long `addToStore`, so without this a slow "copying to
//! the store" looks like a hang. Tracing is privileged and platform-specific,
//! so every failure path degrades to a status string the panel shows rather
//! than aborting the run.
//!
//! * macOS: `fs_usage -w -f filesys nix-daemon` (filters by process name, so it
//!   follows every daemon worker, including ones forked after we attach).
//! * Linux: `strace -f -p <pid>` on the daemon master (`-f` follows the per-
//!   connection workers it forks).

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nix_web_monitor_parser::MonitorState;
use nix_web_monitor_parser::daemon::{DaemonTrace, parse_fs_usage_line, parse_strace_line};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, broadcast};
use tokio::time::{MissedTickBehavior, interval};

use crate::broadcast_deltas;

/// How often the daemon view is recomputed and (if changed) broadcast. Exactly
/// one second so the per-window syscall delta *is* the per-second rate, with no
/// division (and no lossy int/float casts) to compute it.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Pause before re-discovering the daemon after the tracer exits or no daemon is
/// found, so a single-user store (no daemon) or a denied tracer does not spin.
const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Run the daemon syscall probe until the task is aborted.
///
/// Never returns an error: tracing is a best-effort overlay, so any failure
/// becomes a status the panel shows and the loop retries.
pub async fn run_daemon_probe(
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) {
    loop {
        let pids = daemon_pids().await;
        if pids.is_empty() {
            publish_status(
                &monitor,
                &deltas,
                "no nix-daemon running -- single-user store, nothing to trace",
            )
            .await;
            tokio::time::sleep(RETRY_INTERVAL).await;
            continue;
        }

        match spawn_tracer(&pids).await {
            Ok(child) => trace_loop(child, pids, &monitor, &deltas).await,
            Err(reason) => publish_status(&monitor, &deltas, &reason).await,
        }
        // The tracer exited (daemon idle/restarted or attach denied). Surface
        // the idle state and back off before trying to re-attach.
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

/// `nix-daemon` pids, newest last. Empty on a single-user store or if `pgrep` is
/// missing. `pgrep` ships on macOS and Linux, so this needs no extra dependency.
async fn daemon_pids() -> Vec<u32> {
    let Ok(output) = Command::new("pgrep")
        .arg("-f")
        .arg("nix-daemon")
        .output()
        .await
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect()
}

/// Whether the current process is root, so we know whether to wrap the tracer in
/// `sudo -n`. Shells out to `id -u` to avoid a libc dependency for one number.
async fn is_root() -> bool {
    let Ok(output) = Command::new("id").arg("-u").output().await else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).trim() == "0"
}

/// Build the tracer command for this platform.
///
/// Wrapped in `sudo -n` when not already root: the daemon is root-owned, so
/// tracing it needs privilege, and `-n` never prompts -- a user without cached
/// sudo just gets the "needs root" status instead of a hung password prompt.
async fn tracer_command(pids: &[u32]) -> Command {
    let root = is_root().await;
    let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
        // Filter fs_usage by process name so it captures every daemon worker.
        (
            "fs_usage",
            vec![
                "-w".into(),
                "-f".into(),
                "filesys".into(),
                "nix-daemon".into(),
            ],
        )
    } else {
        // strace -f follows the workers the master forks per connection.
        let mut args = vec![
            "-f".into(),
            "-qq".into(),
            "-e".into(),
            "trace=%file,write,fsync,fdatasync".into(),
        ];
        for pid in pids {
            args.push("-p".into());
            args.push(pid.to_string());
        }
        ("strace", args)
    };

    let mut command = if root {
        let mut c = Command::new(program);
        c.args(args);
        c
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(program).args(args);
        c
    };
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Spawn the tracer, or return a human status explaining why it could not start
/// (binary missing, or `sudo -n` refused for lack of privilege).
async fn spawn_tracer(pids: &[u32]) -> Result<Child, String> {
    let mut command = tracer_command(pids).await;
    command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!(
                "syscall tracer not found ({}); daemon view unavailable",
                if cfg!(target_os = "macos") {
                    "fs_usage"
                } else {
                    "strace"
                }
            )
        } else {
            format!("could not start daemon tracer: {error}")
        }
    })
}

/// Read the tracer's output, folding syscalls into a [`DaemonTrace`] and
/// publishing a [`DaemonInfo`] every [`SAMPLE_INTERVAL`]. Returns when the
/// tracer exits.
async fn trace_loop(
    mut child: Child,
    pids: Vec<u32>,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) {
    let Some(stdout) = child.stdout.take() else {
        return;
    };
    // The first stderr line is the most useful failure explanation (e.g.
    // fs_usage's "requires root"); capture it so a denied attach shows a reason.
    let stderr = child.stderr.take();
    let mut lines = BufReader::new(stdout).lines();

    let mut trace = DaemonTrace {
        workers: pids,
        ..DaemonTrace::default()
    };
    // Counts at the previous tick; the difference over the ~1s window is the
    // per-second rate the panel shows.
    let mut last_total: u64 = 0;
    let mut saw_any = false;

    let mut ticker = interval(SAMPLE_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    saw_any = true;
                    if let Some(event) = parse_tracer_line(&line) {
                        trace.record(event);
                    }
                }
                // Stream ended or errored: the tracer is gone.
                Ok(None) | Err(_) => break,
            },
            _ = ticker.tick() => {
                let total = trace.ops.total();
                let ops_per_sec = total.saturating_sub(last_total);
                last_total = total;
                let worker_count = trace.workers.len();
                let status = format!("tracing nix-daemon ({worker_count} workers)");
                let info = trace.info(true, status, ops_per_sec);
                monitor.write().await.set_daemon(info);
                let _ = broadcast_deltas(monitor, deltas).await;
            }
        }
    }

    // The tracer produced nothing and exited: almost always a denied attach.
    if !saw_any {
        let reason = denied_reason(stderr, is_root().await).await;
        publish_status(monitor, deltas, &reason).await;
    }
}

/// Parse one tracer line with the parser for this platform.
fn parse_tracer_line(line: &str) -> Option<nix_web_monitor_parser::daemon::SyscallEvent> {
    if cfg!(target_os = "macos") {
        parse_fs_usage_line(line)
    } else {
        parse_strace_line(line)
    }
}

/// Best explanation for a tracer that started but produced nothing: prefer its
/// own stderr, else a privilege hint.
async fn denied_reason(stderr: Option<tokio::process::ChildStderr>, root: bool) -> String {
    if let Some(stderr) = stderr
        && let Ok(Some(first)) = BufReader::new(stderr).lines().next_line().await
        && !first.trim().is_empty()
    {
        return format!("daemon tracer: {}", first.trim());
    }
    if root {
        "daemon tracer attached but reported no syscalls".to_owned()
    } else {
        "nix-daemon syscall tracing needs root -- run `sudo -v`, then restart, \
         or run nwm under sudo"
            .to_owned()
    }
}

/// Publish a not-tracing [`DaemonInfo`] carrying only a status line.
async fn publish_status(
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
    status: &str,
) {
    let info = DaemonTrace::default().info(false, status.to_owned(), 0);
    monitor.write().await.set_daemon(info);
    let _ = broadcast_deltas(monitor, deltas).await;
}
