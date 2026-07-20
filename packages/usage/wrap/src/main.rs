//! `ix-wrap`: transparent usage-counting exec wrapper (index#3802).
//!
//! nix `withUsage` points each wrapped binary here via `IX_USAGE_SPEC`. The
//! wrapper spawns the real target with inherited stdio and argv0, stays out
//! of the way (ctrl-C reaches the child, SIGTERM is forwarded), and after
//! the child exits appends one line to the local spool: a single `O_APPEND`
//! write, no locks, no SQLite on the hot path. It then exits with the
//! child's status (`128 + signal` for signal deaths).
//!
//! Failure policy: anything telemetry-internal is dropped silently
//! (telemetry must never break the wrapped tool), but a broken spec is a
//! build bug and fails loudly with exit 127 so it cannot masquerade as the
//! tool's own failure.

use std::os::unix::process::{CommandExt as _, ExitStatusExt as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use nix::sys::signal::{SigHandler, Signal};
use serde::Deserialize;

/// Wrapper policy baked by nix `withUsage` into the `IX_USAGE_SPEC` file.
#[derive(Deserialize)]
struct Spec {
    /// Absolute path of the real binary.
    target: String,
    /// Package id recorded in counts.
    pkg: String,
    /// Package version recorded in counts.
    version: String,
    /// `observe` (default) waits and records the exit code; `count-only`
    /// records the invocation and `exec()`s the target so no wrapper
    /// process lingers (for hot-path tools called in tight loops).
    #[serde(default)]
    mode: Mode,
    /// Whether failing invocations keep argv and cwd in the local database
    /// (never uploaded either way).
    #[serde(default = "default_true")]
    errors: bool,
    /// Path to the `ix-usage` CLI for the detached upload kick; absent
    /// disables kicks.
    #[serde(default)]
    uploader: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    #[default]
    Observe,
    CountOnly,
}

const fn default_true() -> bool {
    true
}

/// Spool size above which the wrapper triggers a compaction.
const COMPACT_THRESHOLD_BYTES: u64 = 256 * 1024;
/// Minimum spacing between detached upload kicks. The uploader enforces the
/// real 24h cadence; this only avoids spawning a kick process per
/// invocation.
const KICK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

fn main() {
    let spec = load_spec().unwrap_or_else(|err| {
        eprintln!("ix-wrap: {err}");
        std::process::exit(127)
    });
    match spec.mode {
        Mode::CountOnly => run_count_only(&spec),
        Mode::Observe => run_observe(&spec),
    }
}

fn load_spec() -> Result<Spec, String> {
    let path = std::env::var_os("IX_USAGE_SPEC").ok_or_else(|| {
        "IX_USAGE_SPEC is not set; ix-wrap is only invoked through nix withUsage wrappers"
            .to_owned()
    })?;
    let path = Path::new(&path);
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("reading spec {}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("parsing spec {}: {err}", path.display()))
}

/// The target command with our argv, argv0, environment, and stdio.
fn build_command(spec: &Spec) -> Command {
    let mut args = std::env::args_os();
    let arg0 = args.next();
    let mut command = Command::new(&spec.target);
    command.args(args);
    // The spec must not leak: a target that spawns other wrapped tools would
    // otherwise record them under this spec's package.
    command.env_remove("IX_USAGE_SPEC");
    if let Some(arg0) = arg0 {
        command.arg0(arg0);
    }
    command
}

/// Record the invocation, then replace this process with the target.
fn run_count_only(spec: &Spec) -> ! {
    // Consent bookkeeping and upload kicks are skipped on purpose: this mode
    // exists for tools in tight loops, and observe-mode invocations of other
    // packages carry that bookkeeping for the install.
    record(spec, None, None);
    let err = build_command(spec).exec();
    eprintln!("ix-wrap: exec {}: {err}", spec.target);
    std::process::exit(127)
}

static CHILD_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal(signal: libc::c_int) {
    let pid = CHILD_PID.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: kill(2) is async-signal-safe.
        unsafe {
            libc::kill(pid, signal);
        }
    }
}

/// Spawn, observe, record, and mirror the child's exit status.
fn run_observe(spec: &Spec) -> ! {
    // SIGTERM/SIGHUP forwarding installs before spawn: handler dispositions
    // reset to default across exec, so the child is unaffected. Failure to
    // install a forwarder degrades signal fidelity but must not block the
    // tool, hence the discarded results.
    // SAFETY: the handler only performs an atomic load and kill(2), both
    // async-signal-safe.
    unsafe {
        let _ = nix::sys::signal::signal(Signal::SIGTERM, SigHandler::Handler(forward_signal));
        let _ = nix::sys::signal::signal(Signal::SIGHUP, SigHandler::Handler(forward_signal));
    }

    let started = Instant::now();
    let mut child = match build_command(spec).spawn() {
        Ok(child) => child,
        Err(err) => {
            eprintln!("ix-wrap: spawn {}: {err}", spec.target);
            std::process::exit(127)
        }
    };
    if let Ok(pid) = i32::try_from(child.id()) {
        CHILD_PID.store(pid, Ordering::Relaxed);
    }
    // Ignore INT/QUIT only after the spawn so the child does not inherit the
    // ignore disposition (SIG_IGN survives exec, handlers do not). Ctrl-C
    // then kills the child via the shared foreground process group while we
    // stay alive to record its exit, the same shape `time(1)` uses.
    // SAFETY: SigIgn installs no handler code.
    unsafe {
        let _ = nix::sys::signal::signal(Signal::SIGINT, SigHandler::SigIgn);
        let _ = nix::sys::signal::signal(Signal::SIGQUIT, SigHandler::SigIgn);
    }

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("ix-wrap: waiting for {}: {err}", spec.target);
            std::process::exit(127)
        }
    };
    let Some(exit_code) = status
        .code()
        .or_else(|| status.signal().map(|sig| 128 + sig))
    else {
        eprintln!("ix-wrap: child reported neither exit code nor signal");
        std::process::exit(127)
    };

    record(spec, Some(exit_code), duration_ms(started));
    after_record(spec);
    std::process::exit(exit_code)
}

fn duration_ms(started: Instant) -> Option<u64> {
    u64::try_from(started.elapsed().as_millis()).ok()
}

/// Append one spool line; telemetry failures are dropped by policy.
fn record(spec: &Spec, exit: Option<i32>, duration_ms: Option<u64>) {
    let Ok(ts_ms) = ix_usage_core::spool::now_ms() else {
        return;
    };
    let failed = exit.is_some_and(|code| code != 0);
    let (argv, cwd) = if failed && spec.errors {
        (
            Some(std::env::args().collect()),
            std::env::current_dir()
                .ok()
                .map(|dir| dir.display().to_string()),
        )
    } else {
        (None, None)
    };
    let record = ix_usage_core::spool::Record {
        ts_ms,
        pkg: spec.pkg.clone(),
        version: spec.version.clone(),
        exit,
        duration_ms,
        argv,
        cwd,
    };
    let _ = ix_usage_core::spool::append(&record);
}

/// Post-exit housekeeping, all best-effort: one-time consent bookkeeping,
/// compaction when the spool grows past the threshold, and at most one
/// detached upload kick per [`KICK_INTERVAL`].
fn after_record(spec: &Spec) {
    let _ = ix_usage_core::consent::first_run();
    let Some(state) = ix_usage_core::paths::state_dir() else {
        return;
    };
    if spool_large(&state) {
        let _ = ix_usage_core::store::compact(&state);
    }
    maybe_kick_uploader(spec, &state);
}

fn spool_large(state: &Path) -> bool {
    state
        .join("usage.spool")
        .metadata()
        .is_ok_and(|meta| meta.len() > COMPACT_THRESHOLD_BYTES)
}

fn maybe_kick_uploader(spec: &Spec, state: &Path) {
    let Some(uploader) = &spec.uploader else {
        return;
    };
    // An unreadable consent state counts as no consent, never as yes.
    if !matches!(
        ix_usage_core::consent::resolve().map(|consent| consent.upload),
        Ok(true)
    ) {
        return;
    }
    if !marker_stale(&state.join("upload-kick")) {
        return;
    }
    let mut command = Command::new(uploader);
    command
        .arg("upload")
        .arg("--if-due")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid(2) in the pre-exec child is async-signal-safe; failure
    // (already a session leader) is harmless, stdio is already detached.
    unsafe {
        command.pre_exec(|| {
            let _ = libc::setsid();
            Ok(())
        });
    }
    let _ = command.spawn();
}

/// True when the marker is missing or older than [`KICK_INTERVAL`]. Touches
/// the marker on staleness so concurrent invocations kick once per interval.
fn marker_stale(marker: &Path) -> bool {
    let fresh = std::fs::metadata(marker)
        .and_then(|meta| meta.modified())
        .map(|modified| matches!(modified.elapsed(), Ok(age) if age < KICK_INTERVAL));
    match fresh {
        Ok(true) => false,
        // Missing marker, unreadable mtime, or an mtime in the future: treat
        // as stale and reset it.
        Ok(false) | Err(_) => {
            let _ = std::fs::write(marker, b"");
            true
        }
    }
}
