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
//!
//! The tracer runs only while at least one dashboard client is subscribed to
//! the delta feed: the panel is consumed nowhere else, and macOS allows a
//! single machine-wide ktrace session, so an unwatched tracer starves every
//! other `fs_usage`/`ktrace` consumer for output nobody sees (#2177). The
//! probe parks until the first client connects and, via the babysitter's
//! stdin-EOF kill path, ends the tracer once the last client leaves.

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

/// How often the parked probe re-checks for a first dashboard client.
const CLIENT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Panel status while the probe is parked with no dashboard client connected.
/// Late joiners still see it: every new client is seeded from the monitor
/// snapshot, and their arrival un-parks the probe within a poll interval.
const UNWATCHED_STATUS: &str = "daemon tracing idle -- attaches while the dashboard is open";

/// Run the daemon syscall probe until the task is aborted.
///
/// Never returns an error: tracing is a best-effort overlay, so any failure
/// becomes a status the panel shows and the loop retries.
pub async fn run_daemon_probe(
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) {
    loop {
        // Park until a dashboard client subscribes; see the module doc for why
        // an unwatched tracer must not run.
        if deltas.receiver_count() == 0 {
            publish_status(&monitor, &deltas, UNWATCHED_STATUS).await;
            while deltas.receiver_count() == 0 {
                tokio::time::sleep(CLIENT_POLL_INTERVAL).await;
            }
        }

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
            Ok(child) => {
                if trace_loop(child, pids, &monitor, &deltas).await == TraceEnd::Unwatched {
                    // The gate ended the tracer, not a failure: go straight
                    // back to parking instead of burning the retry pause.
                    continue;
                }
            }
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

/// Shell wrapper that ties the tracer's lifetime to this process.
///
/// The tracer runs as root while the monitor usually does not, so no signal we
/// send can reach it: `kill_on_drop` gets EPERM against the `sudo` child, and a
/// signal that kills the monitor without unwinding (tmux's SIGHUP, SIGKILL)
/// never even fires it. An orphaned `fs_usage` is expensive: it holds the
/// machine's only ktrace session and burns a core against a busy daemon
/// (#2187). So the kill must come from the tracer's own privilege level: a
/// watchdog forked before `exec`ing the tracer holds our stdin pipe, whose EOF
/// is the one signal no death path can suppress (the kernel closes the write
/// end when this process exits, however it exits), and then TERMs the tracer.
/// `$$` names the tracer because `exec` keeps the shell's pid. Details that
/// matter: stdin is dup'd to fd 3 because `&` re-points a background command's
/// stdin at /dev/null, and the watchdog group's stdio is redirected to
/// /dev/null so a tracer exit still closes the stdout pipe [`trace_loop`]
/// watches for EOF.
const TRACER_BABYSITTER: &str = concat!(
    "exec 3<&0\n",
    "{ cat <&3; kill \"$$\"; } >/dev/null 2>&1 &\n",
    "exec \"$0\" \"$@\" 3<&-\n",
);

/// Build the tracer command for this platform.
///
/// Wrapped in `sudo -n` when not already root: the daemon is root-owned, so
/// tracing it needs privilege, and `-n` never prompts -- a user without cached
/// sudo just gets the "needs root" status instead of a hung password prompt.
/// Either way the tracer runs under [`TRACER_BABYSITTER`] with stdin piped, so
/// it dies with this process instead of surviving as an unkillable root orphan.
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
        Command::new("sh")
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg("sh");
        c
    };
    command
        .arg("-c")
        .arg(TRACER_BABYSITTER)
        .arg(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Spawn the tracer, or return a human status explaining why it could not
/// start. A missing tracer binary does not fail the spawn (the shell wrapper
/// starts fine and its `exec` fails); that surfaces through the tracer's
/// stderr as a "command not found" status via [`denied_reason`] instead.
async fn spawn_tracer(pids: &[u32]) -> Result<Child, String> {
    let mut command = tracer_command(pids).await;
    command
        .spawn()
        .map_err(|error| format!("could not start daemon tracer: {error}"))
}

/// Why [`trace_loop`] returned, so the probe knows whether to back off
/// (tracer died) or park immediately (the gate ended it on purpose).
#[derive(Debug, PartialEq, Eq)]
enum TraceEnd {
    /// The tracer exited on its own: daemon restarted, attach denied, or died.
    TracerExited,
    /// The last dashboard client disconnected and the gate ended the tracer.
    Unwatched,
}

/// Read the tracer's output, folding syscalls into a [`DaemonTrace`] and
/// publishing a [`DaemonInfo`] every [`SAMPLE_INTERVAL`]. Returns when the
/// tracer exits, or when the last dashboard client disconnects -- dropping
/// `child` closes its stdin pipe, which trips the babysitter's EOF kill path,
/// the one signal that reaches the root-owned tracer.
async fn trace_loop(
    mut child: Child,
    pids: Vec<u32>,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> TraceEnd {
    let Some(stdout) = child.stdout.take() else {
        return TraceEnd::TracerExited;
    };
    // The first stderr line is the most useful failure explanation (e.g.
    // fs_usage's "requires root"); capture it so a denied attach shows a reason.
    let stderr = child.stderr.take();
    let mut lines = BufReader::new(stdout).lines();

    let mut trace = DaemonTrace::with_workers(pids);
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
                // Gate: end the tracer once nobody is watching (see module
                // doc). Returning drops `child`, whose closed stdin pipe has
                // the babysitter TERM the tracer at its own privilege level.
                if deltas.receiver_count() == 0 {
                    publish_status(monitor, deltas, UNWATCHED_STATUS).await;
                    return TraceEnd::Unwatched;
                }
                let total = trace.ops.total();
                let ops_per_sec = total.saturating_sub(last_total);
                last_total = total;
                let worker_count = trace.workers.len();
                let status = format!("tracing nix-daemon ({worker_count} workers)");
                let info = trace.info(true, status, ops_per_sec, trace.hot_paths(5));
                trace.clear_window();
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
    TraceEnd::TracerExited
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
    let info = DaemonTrace::default().info(false, status.to_owned(), 0, Vec::new());
    monitor.write().await.set_daemon(info);
    let _ = broadcast_deltas(monitor, deltas).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// [`TRACER_BABYSITTER`] with a stand-in tracer and no sudo, stdio wired
    /// exactly as [`tracer_command`] wires it.
    fn wrapped(program: &str, args: &[&str]) -> Command {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(TRACER_BABYSITTER)
            .arg(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// The orphan guard (#2187): when the monitor dies its stdin pipe closes,
    /// and the watchdog must reap the tracer no signal of ours could reach.
    #[tokio::test]
    async fn tracer_dies_when_monitor_stdin_closes() {
        let mut child = wrapped("sleep", &["300"]).spawn().expect("spawn wrapper");
        drop(child.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .expect("tracer should die once stdin closes")
            .expect("wait on tracer");
        // TERMed by the watchdog, not a clean exit.
        assert!(!status.success());
    }

    /// A tracer exiting on its own must still close the stdout pipe (the
    /// lingering watchdog holds /dev/null, not our pipe): stdout EOF is how
    /// [`trace_loop`] detects tracer death.
    #[tokio::test]
    async fn tracer_exit_closes_stdout() {
        let mut child = wrapped("echo", &["done"]).spawn().expect("spawn wrapper");
        let mut stdout = child.stdout.take().expect("stdout piped");
        let mut output = String::new();
        tokio::time::timeout(Duration::from_secs(10), stdout.read_to_string(&mut output))
            .await
            .expect("stdout should reach EOF when the tracer exits")
            .expect("read stdout");
        assert_eq!(output, "done\n");
    }

    /// With no dashboard client subscribed, the probe must park on the
    /// unwatched status without attaching: headless and `serve` runs never
    /// hold the machine-wide ktrace session while nobody watches (#2177).
    #[tokio::test]
    async fn probe_parks_without_clients() {
        let monitor = Arc::new(RwLock::new(MonitorState::default()));
        let (deltas, _) = broadcast::channel(8);
        let probe = tokio::spawn(run_daemon_probe(Arc::clone(&monitor), deltas));
        // One scheduling round is enough for the park status to publish.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let daemon = monitor.read().await.daemon.clone();
        probe.abort();
        assert!(!daemon.tracing);
        assert_eq!(daemon.status, UNWATCHED_STATUS);
    }

    /// Once tracing, the gate must end the loop when the last client is gone
    /// (the babysitter then reaps the tracer; its kill path is covered by
    /// `tracer_dies_when_monitor_stdin_closes`).
    #[tokio::test]
    async fn gate_ends_trace_loop_when_last_client_leaves() {
        let monitor = Arc::new(RwLock::new(MonitorState::default()));
        let (deltas, _) = broadcast::channel(8);
        let child = wrapped("sleep", &["300"]).spawn().expect("spawn wrapper");
        let end = tokio::time::timeout(
            Duration::from_secs(10),
            trace_loop(child, Vec::new(), &monitor, &deltas),
        )
        .await
        .expect("gate should end the loop");
        assert_eq!(end, TraceEnd::Unwatched);
        assert_eq!(monitor.read().await.daemon.status, UNWATCHED_STATUS);
    }
}
