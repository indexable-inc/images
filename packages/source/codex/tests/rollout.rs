//! Parse synthetic Codex session rollouts and check the projected documents.

#![expect(
    clippy::expect_used,
    reason = "tests assert observable parse outcomes"
)]

use std::path::{Path, PathBuf};

use source_codex::CodexHistory;
use source_meta::{Document, SourceAdapter as _};

/// Write one rollout file under a `sessions/YYYY/MM/DD` shard.
fn write_rollout(root: &Path, name: &str, lines: &[String]) -> PathBuf {
    let shard = root.join("2026").join("05").join("31");
    std::fs::create_dir_all(&shard).expect("mkdir shard");
    let path = shard.join(name);
    let mut contents = lines.join("\n");
    contents.push('\n');
    std::fs::write(&path, contents).expect("write rollout");
    path
}

fn session_meta(id: &str, cwd: &str) -> String {
    format!(
        r#"{{"timestamp":"2026-05-31T10:00:00.000Z","type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}","cli_version":"0.134.0"}}}}"#
    )
}

fn documents(history: &CodexHistory) -> Vec<Document> {
    history
        .documents()
        .collect::<Result<Vec<_>, _>>()
        .expect("documents")
}

#[test]
fn full_session_renders_messages_and_folded_tool_calls() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lines = [
        session_meta("s-full", "/home/u/proj"),
        r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"turn_context","payload":{"cwd":"/home/u/proj","model":"gpt-5.5"}}"#.to_owned(),
        // Developer boilerplate: never indexed.
        r#"{"timestamp":"2026-05-31T10:00:02.000Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>\nsandbox stuff\n</permissions instructions>"}]}}"#.to_owned(),
        // User prompt with an injected context block that must be dropped.
        r#"{"timestamp":"2026-05-31T10:00:03.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\ncwd: /home/u/proj\n</environment_context>"},{"type":"input_text","text":"PROMPT-MARKER fix the flaky test"}]}}"#.to_owned(),
        // Encrypted reasoning: nothing embeddable, skipped.
        r#"{"timestamp":"2026-05-31T10:00:04.000Z","type":"response_item","payload":{"type":"reasoning","summary":[],"content":null,"encrypted_content":"gAAAAAB-opaque"}}"#.to_owned(),
        // A tool call whose output arrives on a later line.
        r#"{"timestamp":"2026-05-31T10:00:05.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}","call_id":"call_1"}}"#.to_owned(),
        r#"{"timestamp":"2026-05-31T10:00:06.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"OUTPUT-MARKER 1 passed"}}"#.to_owned(),
        // UI echo of the assistant message: a duplicate, skipped.
        r#"{"timestamp":"2026-05-31T10:00:07.000Z","type":"event_msg","payload":{"type":"agent_message","message":"ANSWER-MARKER done"}}"#.to_owned(),
        r#"{"timestamp":"2026-05-31T10:00:08.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ANSWER-MARKER done"}]}}"#.to_owned(),
        r#"{"timestamp":"2026-05-31T10:00:09.000Z","type":"response_item","payload":{"type":"web_search_call","status":"completed","action":{"type":"search","query":"serde adjacently tagged"}}}"#.to_owned(),
    ];
    write_rollout(temp.path(), "rollout-2026-05-31T10-00-00-s-full.jsonl", &lines);

    let history =
        CodexHistory::open_with(None, Some(temp.path()), "host1", "user1").expect("open");
    let docs = documents(&history);
    assert_eq!(
        docs.len(),
        4,
        "user prompt, tool call, assistant answer, web search: {:?}",
        docs.iter().map(|doc| &doc.external_id).collect::<Vec<_>>()
    );

    let user = &docs[0];
    let body = String::from_utf8(user.body.clone()).expect("utf8");
    assert!(body.contains("PROMPT-MARKER"), "{body}");
    assert!(
        !body.contains("environment_context"),
        "injected context dropped: {body}"
    );
    assert_eq!(user.meta_json["role"], "user");
    assert_eq!(user.meta_json["record_type"], "message");
    assert_eq!(user.meta_json["session_id"], "s-full");
    assert_eq!(user.meta_json["host"], "host1");
    assert_eq!(user.meta_json["user"], "user1");
    assert_eq!(user.meta_json["cwd"], "/home/u/proj");
    assert_eq!(user.meta_json["project"], "/home/u/proj");
    assert!(user.external_id.starts_with("codex:s-full:sha256:"));
    let expected_ts = chrono::DateTime::parse_from_rfc3339("2026-05-31T10:00:03.000Z")
        .expect("rfc3339")
        .timestamp();
    assert_eq!(user.meta_json["timestamp"], expected_ts);

    let call = &docs[1];
    let body = String::from_utf8(call.body.clone()).expect("utf8");
    assert!(body.contains("[tool_use exec_command]"), "{body}");
    assert!(body.contains("cargo test"), "call input present: {body}");
    assert!(
        body.contains("[tool_result] OUTPUT-MARKER 1 passed"),
        "result folded in: {body}"
    );
    assert_eq!(call.meta_json["record_type"], "function_call");
    assert_eq!(call.meta_json["role"], "assistant");
    assert_eq!(call.meta_json["tool_name"], "exec_command");
    assert_eq!(call.meta_json["model"], "gpt-5.5");

    let answer = &docs[2];
    assert_eq!(answer.meta_json["role"], "assistant");
    assert_eq!(answer.body, b"ANSWER-MARKER done");

    let search = &docs[3];
    assert_eq!(search.meta_json["record_type"], "web_search_call");
    assert_eq!(search.body, b"[web_search] serde adjacently tagged");
}

#[test]
fn resumed_session_replay_dedupes_against_the_original() {
    let temp = tempfile::tempdir().expect("tempdir");
    let original = [
        session_meta("s-orig", "/home/u/proj"),
        r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"REPLAYED-MARKER answer"}]}}"#.to_owned(),
    ];
    write_rollout(
        temp.path(),
        "rollout-2026-05-31T10-00-00-s-orig.jsonl",
        &original,
    );
    // The resumed file opens with its own meta, then replays the original
    // session's history — original `session_meta` included — with FRESH
    // timestamps (observed Codex behavior), then continues.
    let resumed = [
        session_meta("s-resume", "/home/u/proj"),
        session_meta("s-orig", "/home/u/proj"),
        r#"{"timestamp":"2026-05-31T11:30:00.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"REPLAYED-MARKER answer"}]}}"#.to_owned(),
        r#"{"timestamp":"2026-05-31T11:30:05.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"NEW-MARKER continue"}]}}"#.to_owned(),
    ];
    write_rollout(
        temp.path(),
        "rollout-2026-05-31T11-30-00-s-resume.jsonl",
        &resumed,
    );

    let history =
        CodexHistory::open_with(None, Some(temp.path()), "host1", "user1").expect("open");
    let docs = documents(&history);
    let replayed: Vec<_> = docs
        .iter()
        .filter(|doc| doc.body.starts_with(b"REPLAYED-MARKER"))
        .collect();
    assert_eq!(
        replayed.len(),
        1,
        "the replayed item keys to the same id and dedupes"
    );
    assert_eq!(replayed[0].meta_json["session_id"], "s-orig");
    assert!(
        docs.iter().any(|doc| doc.body.starts_with(b"NEW-MARKER")),
        "the resumed session's new turn is still indexed"
    );
}

#[test]
fn orphan_tool_output_renders_standalone() {
    // An output whose call never appears (a truncated rollout) must still
    // surface its content rather than vanish.
    let temp = tempfile::tempdir().expect("tempdir");
    let lines = [
        session_meta("s-orphan", "/home/u/proj"),
        r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_missing","output":"ORPHAN-OUTPUT"}}"#.to_owned(),
    ];
    write_rollout(
        temp.path(),
        "rollout-2026-05-31T10-00-00-s-orphan.jsonl",
        &lines,
    );

    let history =
        CodexHistory::open_with(None, Some(temp.path()), "host1", "user1").expect("open");
    let docs = documents(&history);
    assert_eq!(docs.len(), 1);
    let body = String::from_utf8(docs[0].body.clone()).expect("utf8");
    assert!(body.contains("[tool_result] ORPHAN-OUTPUT"), "{body}");
    assert_eq!(docs[0].meta_json["role"], "tool");
    assert_eq!(docs[0].meta_json["record_type"], "function_call_output");
}

/// End-to-end hygiene proof: a rollout whose tool output carries a fake
/// credential (constructed at test time — never a real key), ANSI escapes, a
/// base64-ish blob, and a giant log comes out of the adapter's [`Document`]
/// body sanitized, and `content_hash` is computed over the sanitized bytes.
#[test]
fn fake_secret_in_rollout_is_redacted_end_to_end() {
    let fake_key = format!("lin_api_{}", "Ab0".repeat(13));
    let big_output = format!(
        "\u{1b}[1mLOG-HEAD\u{1b}[0m {}\ncurl -H \"Authorization: {fake_key}\"\n{}LOG-TAIL",
        "QUJD+/=a".repeat(40),
        "one line of CI output\n".repeat(600),
    );
    let output_line = serde_json::json!({
        "timestamp": "2026-05-31T10:00:02.000Z",
        "type": "response_item",
        "payload": {"type": "function_call_output", "call_id": "call_ci", "output": big_output},
    });

    let temp = tempfile::tempdir().expect("tempdir");
    let lines = [
        session_meta("s-hygiene", "/home/u/proj"),
        r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"./ci.sh\"}","call_id":"call_ci"}}"#.to_owned(),
        output_line.to_string(),
    ];
    write_rollout(
        temp.path(),
        "rollout-2026-05-31T10-00-00-s-hygiene.jsonl",
        &lines,
    );

    let history =
        CodexHistory::open_with(None, Some(temp.path()), "host1", "user1").expect("open");
    let docs = documents(&history);
    assert_eq!(docs.len(), 1, "call and output fold into one document");
    let document = &docs[0];
    let body = String::from_utf8(document.body.clone()).expect("utf8 body");

    assert!(
        !body.contains(&fake_key),
        "the raw key must never be embedded: {body}"
    );
    assert!(body.contains("[redacted:linear_api_key]"), "{body}");
    assert!(!body.contains('\u{1b}'), "ANSI escapes stripped: {body}");
    assert!(body.contains("[blob 320 chars]"), "{body}");
    assert!(
        body.contains("[truncated"),
        "the giant output is capped: {} chars",
        body.chars().count()
    );
    assert!(body.contains("LOG-TAIL"), "the tail survives the cap");
    assert_eq!(
        document.content_hash,
        source_meta::hash_body(&document.body),
        "content_hash is computed AFTER sanitation, over the embedded bytes"
    );
}

#[test]
fn symlinked_rollout_is_skipped_and_missing_dir_is_empty() {
    let temp = tempfile::tempdir().expect("tempdir");
    // Stands in for a sensitive target a privileged walk must not follow.
    let secret = temp.path().join("secret-target");
    std::fs::write(
        &secret,
        format!(
            "{}\n{}\n",
            session_meta("s-leak", "/"),
            r#"{"timestamp":"2026-05-31T10:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"LEAK-MARKER"}]}}"#
        ),
    )
    .expect("write secret");
    let sessions = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions).expect("mkdir sessions");
    std::os::unix::fs::symlink(&secret, sessions.join("leak.jsonl")).expect("symlink");

    let history = CodexHistory::open_with(None, Some(&sessions), "host1", "user1").expect("open");
    assert!(
        history.is_empty(),
        "the symlinked rollout must not be collected"
    );

    let missing = temp.path().join("nonexistent-sessions");
    let history = CodexHistory::open_with(None, Some(&missing), "host1", "user1").expect("open");
    assert!(history.is_empty(), "a missing sessions dir yields nothing");
}
