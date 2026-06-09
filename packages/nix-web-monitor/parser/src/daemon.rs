//! Live `nix-daemon` syscall view.
//!
//! The internal-json stream the monitor parses elsewhere is the daemon's work
//! *as the client sees it*: it goes silent during a single long `addToStore`
//! (writing a path, or hard-linking every file when `auto-optimise-store` is on),
//! which is exactly when a build looks hung. To see *into* that, the server taps
//! the daemon's filesystem syscalls with a platform tracer (`fs_usage` on macOS,
//! `strace` on Linux) and folds them into the small, wire-friendly
//! [`DaemonInfo`] the UI renders. This module owns the pure, testable pieces:
//! the per-tracer line parsers and the rolling aggregator. Spawning the tracer
//! (privileged, platform-specific, best-effort) lives in the server.

use serde::{Deserialize, Serialize};

/// A class of filesystem syscall.
///
/// Grouped so the UI shows what kind of work the daemon is doing rather than a
/// raw syscall histogram: `Link`/`Rename` dominate store optimisation,
/// `Write`/`Fsync` dominate writing a new path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpClass {
    Link,
    Rename,
    Open,
    Write,
    Fsync,
    Stat,
    Unlink,
    Other,
}

impl OpClass {
    /// Classify a syscall by name. Handles the `*at` and `p*`/`*64` variants the
    /// two tracers emit (`openat`, `pwrite`, `lstat64`, `fdatasync`, ...).
    #[must_use]
    pub fn classify(syscall: &str) -> Self {
        // Strip a trailing `64`/`_nocancel` so `stat64` / `open_nocancel` match.
        let name = syscall
            .trim_end_matches("_nocancel")
            .trim_end_matches("64");
        match name {
            "link" | "linkat" | "clonefile" | "clonefileat" => Self::Link,
            "rename" | "renameat" | "renameatx_np" => Self::Rename,
            "unlink" | "unlinkat" | "rmdir" => Self::Unlink,
            "fsync" | "fdatasync" => Self::Fsync,
            "write" | "pwrite" | "writev" | "pwritev" => Self::Write,
            "open" | "openat" => Self::Open,
            "stat" | "lstat" | "fstat" | "fstatat" | "access" | "getattrlist" | "readlink"
            | "readlinkat" => Self::Stat,
            _ => Self::Other,
        }
    }
}

/// Per-class syscall counts. Cumulative since the tracer started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonOps {
    pub link: u64,
    pub rename: u64,
    pub open: u64,
    pub write: u64,
    pub fsync: u64,
    pub stat: u64,
    pub unlink: u64,
    pub other: u64,
}

impl DaemonOps {
    /// Count one syscall against its class.
    pub const fn bump(&mut self, op: OpClass) {
        let slot = match op {
            OpClass::Link => &mut self.link,
            OpClass::Rename => &mut self.rename,
            OpClass::Open => &mut self.open,
            OpClass::Write => &mut self.write,
            OpClass::Fsync => &mut self.fsync,
            OpClass::Stat => &mut self.stat,
            OpClass::Unlink => &mut self.unlink,
            OpClass::Other => &mut self.other,
        };
        *slot = slot.saturating_add(1);
    }

    /// Total across all classes.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.link
            .saturating_add(self.rename)
            .saturating_add(self.open)
            .saturating_add(self.write)
            .saturating_add(self.fsync)
            .saturating_add(self.stat)
            .saturating_add(self.unlink)
            .saturating_add(self.other)
    }
}

/// One observed daemon syscall: its class and, when the tracer reported one, the
/// path it touched (preferring `/nix/store` paths, which are the interesting
/// ones).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyscallEvent {
    pub op: OpClass,
    pub path: Option<String>,
}

/// Wire-friendly snapshot of the daemon syscall view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonInfo {
    /// Whether a tracer is attached and producing events.
    pub tracing: bool,
    /// Human-readable state when not tracing.
    ///
    /// "no nix-daemon (single-user store)", "syscall tracing needs root", an
    /// error, ...; so the panel always explains itself instead of sitting blank.
    pub status: String,
    /// `nix-daemon` worker pids currently being traced.
    pub workers: Vec<u32>,
    /// Cumulative syscalls by class since tracing started.
    pub ops: DaemonOps,
    /// Recent syscall rate, in events over the last one-second window.
    ///
    /// Integer (not `f64`) so the whole snapshot stays `Eq`, which the
    /// delta-dedup in the state machine relies on; the one-second window means
    /// the per-window count *is* the per-second rate, with no division.
    pub ops_per_sec: u64,
    /// The most recent path the daemon touched, for a "currently working on"
    /// readout. `None` until a path-bearing syscall is seen.
    pub current_path: Option<String>,
}

/// Rolling aggregator that folds [`SyscallEvent`]s into a [`DaemonInfo`].
///
/// Kept separate from the wire type so the server can mutate it freely and
/// publish a clone on a timer. `ops` is cumulative; `ops_per_sec` is computed by
/// the server from the cumulative total between samples.
#[derive(Clone, Debug, Default)]
pub struct DaemonTrace {
    pub ops: DaemonOps,
    pub workers: Vec<u32>,
    pub current_path: Option<String>,
}

impl DaemonTrace {
    /// Fold one parsed syscall event into the running totals.
    pub fn record(&mut self, event: SyscallEvent) {
        self.ops.bump(event.op);
        if let Some(path) = event.path {
            self.current_path = Some(path);
        }
    }

    /// Project the current totals into a wire [`DaemonInfo`]. `ops_per_sec` is
    /// supplied by the caller, which alone knows the wall-clock between samples.
    #[must_use]
    pub fn info(&self, tracing: bool, status: String, ops_per_sec: u64) -> DaemonInfo {
        DaemonInfo {
            tracing,
            status,
            workers: self.workers.clone(),
            ops: self.ops,
            ops_per_sec,
            current_path: self.current_path.clone(),
        }
    }
}

/// Pick the most interesting path from a tracer line's tokens.
///
/// Prefers the last `/nix/store/...` token (the store path being written or
/// linked), else the last absolute-path token; keeps relative and `/dev` noise
/// out of the readout.
fn best_path<'a>(tokens: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut store: Option<&str> = None;
    let mut any_abs: Option<&str> = None;
    for token in tokens {
        let cleaned = token.trim_matches(['"', ',', '\'']);
        if cleaned.starts_with("/nix/store/") {
            store = Some(cleaned);
        } else if cleaned.starts_with('/') && cleaned.len() > 1 {
            any_abs = Some(cleaned);
        }
    }
    store.or(any_abs).map(ToOwned::to_owned)
}

/// Parse one `fs_usage -w -f filesys` line (macOS).
///
/// The format is whitespace-columned: `HH:MM:SS.uuuuuu  <syscall>  <args/paths>
/// <duration> <process>`. We take the first non-timestamp token as the syscall
/// name and scan the remaining tokens for the best path. Lines that are headers
/// or lack a recognizable syscall yield `None`.
#[must_use]
pub fn parse_fs_usage_line(line: &str) -> Option<SyscallEvent> {
    let mut tokens = line.split_whitespace();
    let first = tokens.next()?;
    // The leading token is a `HH:MM:SS.uuuuuu` timestamp; anything else is a
    // header/banner line we skip.
    let stamp = first.as_bytes();
    if stamp.len() < 8 || stamp[2] != b':' || stamp[5] != b':' {
        return None;
    }
    let syscall = tokens.next()?;
    if !syscall.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let op = OpClass::classify(syscall);
    // Re-scan the whole line for a path; fs_usage puts paths after the syscall.
    let path = best_path(line.split_whitespace().skip(2));
    Some(SyscallEvent { op, path })
}

/// Parse one `strace -f` line (Linux).
///
/// The relevant form is `[pid 123] syscall(arg, "path", ...) = ret`. We take the
/// token before the first `(` as the syscall name and the first quoted argument
/// (or any `/nix/store` token) as the path. Resumed (`<... resumed>`), unfinished
/// (`<unfinished ...>`), and signal lines yield `None`.
#[must_use]
pub fn parse_strace_line(line: &str) -> Option<SyscallEvent> {
    let line = line.trim_start();
    // Drop an optional `[pid 1234] ` prefix.
    let rest = match line.strip_prefix("[pid ") {
        Some(after) => after.split_once("] ").map(|(_, rest)| rest)?,
        None => line,
    };
    // The syscall name is the identifier before the first `(`.
    let paren = rest.find('(')?;
    let name = rest[..paren].trim();
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    let op = OpClass::classify(name);
    // First double-quoted argument is the path for the file syscalls we trace.
    let path = rest[paren + 1..]
        .split('"')
        .nth(1)
        .filter(|p| p.starts_with('/'))
        .map(ToOwned::to_owned);
    Some(SyscallEvent { op, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_handles_variants() {
        assert_eq!(OpClass::classify("openat"), OpClass::Open);
        assert_eq!(OpClass::classify("open_nocancel"), OpClass::Open);
        assert_eq!(OpClass::classify("lstat64"), OpClass::Stat);
        assert_eq!(OpClass::classify("link"), OpClass::Link);
        assert_eq!(OpClass::classify("clonefile"), OpClass::Link);
        assert_eq!(OpClass::classify("rename"), OpClass::Rename);
        assert_eq!(OpClass::classify("fdatasync"), OpClass::Fsync);
        assert_eq!(OpClass::classify("pwrite"), OpClass::Write);
        assert_eq!(OpClass::classify("getpid"), OpClass::Other);
    }

    #[test]
    fn ops_bump_and_total() {
        let mut ops = DaemonOps::default();
        ops.bump(OpClass::Link);
        ops.bump(OpClass::Link);
        ops.bump(OpClass::Fsync);
        assert_eq!(ops.link, 2);
        assert_eq!(ops.fsync, 1);
        assert_eq!(ops.total(), 3);
    }

    #[test]
    fn parses_fs_usage_link_line() {
        let line = "22:35:01.123456  link              \
            /nix/store/.tmp-link-abc -> /nix/store/aaaa-foo   0.000030   nix-daemon.31415";
        let event = parse_fs_usage_line(line).expect("a syscall line parses");
        assert_eq!(event.op, OpClass::Link);
        assert_eq!(
            event.path.as_deref(),
            Some("/nix/store/aaaa-foo"),
            "prefers the last /nix/store path"
        );
    }

    #[test]
    fn fs_usage_rejects_banner_lines() {
        assert!(parse_fs_usage_line("").is_none());
        assert!(parse_fs_usage_line("  Command must be run as root").is_none());
        assert!(parse_fs_usage_line("PROCESS   TYPE   PATHNAME").is_none());
    }

    #[test]
    fn parses_strace_lines() {
        let with_pid = r#"[pid 31415] openat(AT_FDCWD, "/nix/store/aaaa-foo", O_RDONLY) = 3"#;
        let event = parse_strace_line(with_pid).expect("strace line parses");
        assert_eq!(event.op, OpClass::Open);
        assert_eq!(event.path.as_deref(), Some("/nix/store/aaaa-foo"));

        let fsync = "fsync(7) = 0";
        let event = parse_strace_line(fsync).expect("fsync parses");
        assert_eq!(event.op, OpClass::Fsync);
        assert_eq!(event.path, None);

        assert!(parse_strace_line("+++ exited with 0 +++").is_none());
        assert!(parse_strace_line(r"[pid 1] <... read resumed>) = 0").is_none());
    }

    #[test]
    fn trace_records_and_projects() {
        let mut trace = DaemonTrace {
            workers: vec![10, 11],
            ..DaemonTrace::default()
        };
        trace.record(SyscallEvent {
            op: OpClass::Write,
            path: Some("/nix/store/x".to_owned()),
        });
        trace.record(SyscallEvent {
            op: OpClass::Fsync,
            path: None,
        });
        let info = trace.info(true, "tracing".to_owned(), 42);
        assert!(info.tracing);
        assert_eq!(info.ops.write, 1);
        assert_eq!(info.ops.fsync, 1);
        assert_eq!(info.ops.total(), 2);
        assert_eq!(info.current_path.as_deref(), Some("/nix/store/x"));
        assert_eq!(info.workers, vec![10, 11]);
        assert_eq!(info.ops_per_sec, 42);
    }
}
