//! Fixture-backed tests for the Claude transcript parser and document grain.

use std::path::PathBuf;

use claude_history::ClaudeHistoryExport;
use search_meta::{Source, SourceAdapter};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn parses_messages_and_tags_them() {
    let export = ClaudeHistoryExport::open_with(&fixtures(), "test-host", "tester").expect("open");
    assert_eq!(export.source(), Source::new("claude_history"));

    let docs: Vec<_> = export
        .documents()
        .collect::<Result<_, _>>()
        .expect("documents");
    // 1 user + 1 assistant message; the title-only line carries no message and
    // is skipped.
    assert_eq!(docs.len(), 2);

    let user_doc = docs
        .iter()
        .find(|doc| doc.meta_json["role"] == "user")
        .expect("user doc");
    assert_eq!(user_doc.meta_json["source"], "claude_history");
    assert_eq!(user_doc.meta_json["host"], "test-host");
    assert_eq!(user_doc.meta_json["user"], "tester");
    assert_eq!(user_doc.meta_json["project"], "/Users/tester/Projects/demo");
    assert_eq!(user_doc.meta_json["session_id"], "sess1");
    assert_eq!(user_doc.external_id, "claude:sess1:u1");
    // The content_hash is the sha256 of the exact embedded bytes.
    assert_eq!(user_doc.content_hash, search_meta::hash_body(&user_doc.body));

    let assistant_doc = docs
        .iter()
        .find(|doc| doc.meta_json["role"] == "assistant")
        .expect("assistant doc");
    assert_eq!(assistant_doc.meta_json["model"], "claude-opus-4-8");
    assert_eq!(assistant_doc.meta_json["tool_name"], "Read");
    assert_eq!(assistant_doc.meta_json["output_tokens"], 42);
    // Thinking, text, and tool_use are all rendered into the embedded body.
    let body = String::from_utf8(assistant_doc.body.clone()).expect("utf8");
    assert!(body.contains("let me think about rust"));
    assert!(body.contains("Sure, here is how you do it in rust."));
    assert!(body.contains("[tool_use Read]"));
}

#[test]
fn reingest_is_stable() {
    // Same input twice yields identical ids and hashes, so a re-ingest of an
    // unchanged transcript is a no-op for the content-hash reconcile.
    let first = ClaudeHistoryExport::open_with(&fixtures(), "h", "u").expect("open");
    let second = ClaudeHistoryExport::open_with(&fixtures(), "h", "u").expect("open");

    let ids = |export: &ClaudeHistoryExport| -> Vec<(String, String)> {
        export
            .documents()
            .map(|doc| {
                let doc = doc.expect("document");
                (doc.external_id, doc.content_hash)
            })
            .collect()
    };
    assert_eq!(ids(&first), ids(&second));
}
