//! Parse the sample Codex history and check the projected documents.

use source_codex::CodexHistory;
use source_meta::{Source, SourceAdapter as _};

fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample.jsonl")
}

#[test]
fn parses_every_prompt_skipping_blank_lines() {
    let history = CodexHistory::open_with(&fixture(), "host1", "user1").expect("parse");
    // Four prompt lines, one blank line skipped.
    assert_eq!(history.len(), 4);
    assert!(!history.is_empty());
}

#[test]
fn external_ids_are_content_stable_not_positional() {
    let history = CodexHistory::open_with(&fixture(), "host1", "user1").expect("parse");
    let ids: Vec<String> = history
        .documents()
        .map(|doc| doc.expect("document").external_id)
        .collect();
    assert_eq!(ids.len(), 4);
    // All distinct, and keyed on session + timestamp + content hash so Codex
    // history compaction (which shifts file positions) never re-keys a prompt.
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 4);
    assert!(ids[0].starts_with("codex:019cfa3f-a908-71b0-98f0-7ecb3874a8db:1773725019:sha256:"));
    assert!(ids[1].starts_with("codex:019cfa3f-a908-71b0-98f0-7ecb3874a8db:1773725086:sha256:"));
    assert!(ids[2].starts_with("codex:019d0102-1111-2222-3333-444455556666:1773726000:sha256:"));
    // The last prompt carried no timestamp, so the ts segment is `na`.
    assert!(ids[3].starts_with("codex:019d0102-1111-2222-3333-444455556666:na:sha256:"));
}

#[test]
fn documents_carry_source_and_tags() {
    let history = CodexHistory::open_with(&fixture(), "host1", "user1").expect("parse");
    assert_eq!(history.source(), Source::new("codex"));

    let docs: Vec<_> = history.documents().map(|doc| doc.expect("document")).collect();
    let first = &docs[0];
    assert_eq!(first.meta_json["source"], "codex");
    assert_eq!(first.meta_json["host"], "host1");
    assert_eq!(first.meta_json["user"], "user1");
    assert_eq!(first.meta_json["session_id"], "019cfa3f-a908-71b0-98f0-7ecb3874a8db");
    assert_eq!(first.meta_json["timestamp"], 1_773_725_019_i64);
    assert_eq!(first.content_hash, first.meta_json["content_hash"]);
    assert_eq!(first.body, b"clean up the README and make it succinct");

    // The last prompt had no `ts`, so the timestamp tag is omitted, not null.
    let last = &docs[3];
    assert!(last.meta_json.get("timestamp").is_none());
}
