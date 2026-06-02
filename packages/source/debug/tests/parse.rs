//! Behavioural tests for the Claude debug-log adapter: what it indexes, what it
//! tags, and the symlink-skip security property.

use std::fs;

use source_debug::{DebugLogs, SOURCE_TAG};
use source_meta::SourceAdapter;

/// A real `*.txt` debug log is indexed as one document tagged with the session
/// id, host, and user; an empty log and a non-`.txt` file are skipped.
#[test]
fn indexes_real_txt_logs_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("sess-1.txt"), b"2026-06-02T17:20:01.816Z [DEBUG] hello\n")
        .expect("write log");
    fs::write(dir.path().join("empty.txt"), b"   \n").expect("write empty");
    fs::write(dir.path().join("notes.md"), b"not a debug log").expect("write md");

    let logs = DebugLogs::open_with(dir.path(), "hydra", "andrew").expect("open");
    assert_eq!(logs.len(), 1, "only the one non-empty .txt is indexed");

    let docs: Vec<_> = logs.documents().map(|d| d.expect("doc")).collect();
    let doc = &docs[0];
    assert_eq!(doc.external_id, "claude_debug:sess-1");
    assert_eq!(doc.meta_json["source"], SOURCE_TAG);
    assert_eq!(doc.meta_json["session_id"], "sess-1");
    assert_eq!(doc.meta_json["host"], "hydra");
    assert_eq!(doc.meta_json["user"], "andrew");
    assert!(String::from_utf8_lossy(&doc.body).contains("hello"));
}

/// A missing debug dir is normal (no `--debug` runs) and yields an empty set.
#[test]
fn missing_dir_is_empty_not_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let logs = DebugLogs::open_with(&dir.path().join("nope"), "h", "u").expect("open missing");
    assert!(logs.is_empty());
}

/// A symlinked log (the privileged confused-deputy threat) is skipped, never
/// followed: a planted `*.txt -> secret` does not exfiltrate the target.
#[cfg(unix)]
#[test]
fn skips_symlinked_logs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secret = dir.path().join("secret");
    fs::write(&secret, b"TOP SECRET ROOT FILE").expect("write secret");
    std::os::unix::fs::symlink(&secret, dir.path().join("planted.txt")).expect("symlink");

    let logs = DebugLogs::open_with(dir.path(), "h", "u").expect("open");
    assert_eq!(logs.len(), 0, "a symlinked .txt must be skipped, not followed");
    let bodies: Vec<_> =
        logs.documents().map(|d| String::from_utf8_lossy(&d.expect("doc").body).into_owned()).collect();
    assert!(!bodies.iter().any(|b| b.contains("SECRET")), "secret target must not be read");
}
