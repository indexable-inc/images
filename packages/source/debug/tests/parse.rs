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
    fs::write(
        dir.path().join("sess-1.txt"),
        b"2026-06-02T17:20:01.816Z [DEBUG] hello\n",
    )
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
    assert_eq!(
        logs.len(),
        0,
        "a symlinked .txt must be skipped, not followed"
    );
    let bodies: Vec<_> = logs
        .documents()
        .map(|d| String::from_utf8_lossy(&d.expect("doc").body).into_owned())
        .collect();
    assert!(
        !bodies.iter().any(|b| b.contains("SECRET")),
        "secret target must not be read"
    );
}

/// A debug log carrying terminal noise, a credential-shaped token, and a hex
/// blob (all constructed at test time — never a real secret) is sanitized
/// before hashing and embedding, and `content_hash` covers the sanitized bytes
/// so a re-sync replaces previously ingested raw bodies.
#[test]
fn debug_log_body_is_sanitized_and_redacted() {
    let fake_token = format!("ghp_{}", "Ab1".repeat(12));
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("sess-2.txt"),
        format!(
            "2026-06-02T17:20:01.816Z [DEBUG] \u{1b}[1mauth\u{1b}[0m token={fake_token}\n\
             2026-06-02T17:20:02.000Z [DEBUG] payload {}\n",
            "deadbeef".repeat(40),
        ),
    )
    .expect("write log");

    let logs = DebugLogs::open_with(dir.path(), "h", "u").expect("open");
    let docs: Vec<_> = logs.documents().map(|d| d.expect("doc")).collect();
    assert_eq!(docs.len(), 1);
    let body = String::from_utf8(docs[0].body.clone()).expect("utf8 body");

    assert!(
        !body.contains(&fake_token),
        "the raw token must never be embedded: {body}"
    );
    assert!(body.contains("[redacted:github_token]"), "{body}");
    assert!(!body.contains('\u{1b}'), "ANSI escapes stripped: {body}");
    assert!(body.contains("[blob 320 chars]"), "{body}");
    assert_eq!(
        docs[0].content_hash,
        source_meta::hash_body(&docs[0].body),
        "content_hash is computed AFTER sanitation"
    );
}
