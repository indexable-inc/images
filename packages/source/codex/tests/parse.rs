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
fn external_ids_are_per_session_sequential() {
    let history = CodexHistory::open_with(&fixture(), "host1", "user1").expect("parse");
    let ids: Vec<String> = history
        .documents()
        .map(|doc| doc.expect("document").external_id)
        .collect();
    assert_eq!(
        ids,
        vec![
            "codex:019cfa3f-a908-71b0-98f0-7ecb3874a8db:0",
            "codex:019cfa3f-a908-71b0-98f0-7ecb3874a8db:1",
            "codex:019d0102-1111-2222-3333-444455556666:0",
            "codex:019d0102-1111-2222-3333-444455556666:1",
        ]
    );
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
