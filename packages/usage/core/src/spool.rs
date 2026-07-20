//! Hot-path recording: one `O_APPEND` write per invocation.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One tool invocation, JSON-encoded as a single spool line.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Unix milliseconds when the record was written.
    pub ts_ms: i64,
    /// Package id (the nix package name, not the binary basename).
    pub pkg: String,
    /// Package version.
    pub version: String,
    /// Child exit code (`128 + signal` for signal deaths); `None` when the
    /// wrapper exec'd the target without observing it (count-only mode).
    pub exit: Option<i32>,
    /// Wall time from spawn to exit; `None` in count-only mode.
    pub duration_ms: Option<u64>,
    /// Full argv of a failing invocation. Stays in the local database; the
    /// upload path cannot read it (see [`crate::payload`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// Working directory of a failing invocation, local-only like `argv`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Append one record to the spool at the default location.
///
/// Succeeds without writing when no state dir exists (nix sandbox, no HOME).
///
/// # Errors
/// Any filesystem error. Wrapper callers swallow it: telemetry must never
/// break the wrapped tool.
pub fn append(record: &Record) -> std::io::Result<()> {
    crate::paths::spool_path().map_or(Ok(()), |path| append_at(&path, record))
}

/// Append one record to the spool at `path` (one `O_APPEND` write; atomic
/// for lines under `PIPE_BUF`).
///
/// # Errors
/// Any filesystem error.
pub fn append_at(path: &Path, record: &Record) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_vec(record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&line)
}

/// Unix milliseconds now.
///
/// # Errors
/// Fails when the system clock reads before the Unix epoch or beyond `i64`
/// milliseconds.
pub fn now_ms() -> std::io::Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    i64::try_from(elapsed.as_millis()).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::{Record, append_at};

    fn record() -> Record {
        Record {
            ts_ms: 1_700_000_000_000,
            pkg: "demo".to_owned(),
            version: "1.0".to_owned(),
            exit: Some(3),
            duration_ms: Some(12),
            argv: Some(vec!["demo".to_owned(), "--flag".to_owned()]),
            cwd: Some("/tmp".to_owned()),
        }
    }

    #[test]
    fn round_trips_as_json_line() {
        let line = serde_json::to_string(&record()).expect("serialize");
        let back: Record = serde_json::from_str(&line).expect("parse");
        assert_eq!(back, record());
    }

    #[test]
    fn appends_one_line_per_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage.spool");
        append_at(&path, &record()).expect("first append");
        append_at(&path, &record()).expect("second append");
        let text = std::fs::read_to_string(&path).expect("read spool");
        assert_eq!(text.lines().count(), 2);
    }
}
