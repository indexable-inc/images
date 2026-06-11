//! Integration tests against a tiny synthetic, anonymized fixture export.
//!
//! The fixture lives under `tests/fixture/` and deliberately exercises the
//! hard cases: a thread whose root is in one day file and whose reply is in
//! another (cross-file assembly), a standalone message, a bot message with no
//! top-level text, and `channel_join` / `channel_leave` system messages that
//! must be dropped. No private export data is used.

use std::{collections::HashMap, path::PathBuf};

use source_meta::{Document, SourceAdapter};
use source_slack::SlackExport;

/// Absolute path to the synthetic fixture export root.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

/// Collect all documents, asserting none errored.
fn collect_documents() -> Vec<Document> {
    let export = SlackExport::open(&fixture_dir()).expect("open synthetic fixture");
    export
        .documents()
        .collect::<Result<Vec<_>, _>>()
        .expect("iterate fixture documents without error")
}

/// Index documents by external id for targeted assertions.
fn by_external_id(documents: &[Document]) -> HashMap<&str, &Document> {
    documents
        .iter()
        .map(|doc| (doc.external_id.as_str(), doc))
        .collect()
}

/// The cross-file thread (root in day 1, reply in day 2) must assemble into one
/// document with both messages, the right id, resolved mentions, and a content
/// hash that matches its body.
#[test]
fn cross_file_thread_assembled_into_one_document() {
    let documents = collect_documents();
    let index = by_external_id(&documents);

    let thread = index
        .get("slack:C0SYNTH001:1735689700.000200")
        .expect("cross-file thread document present");

    // Root + the day-2 reply == 2 messages.
    assert_eq!(
        thread.meta_json["message_count"], 2,
        "root + cross-file reply"
    );

    // external_id == slack:<cid>:<thread_ts>.
    assert_eq!(thread.external_id, "slack:C0SYNTH001:1735689700.000200");
    assert_eq!(thread.meta_json["thread_ts"], "1735689700.000200");

    let body = String::from_utf8(thread.body.clone()).expect("body is utf-8");

    // Mention <@U0SYNTHBBB> resolved to a display/real name, never left as raw id.
    assert!(
        body.contains("@Bob Builder"),
        "mention resolved to name, body:\n{body}"
    );
    assert!(
        !body.contains("U0SYNTHBBB"),
        "raw mention id must not survive, body:\n{body}"
    );

    // Both messages present, in ascending ts order (root question before reply).
    let root_pos = body.find("what docker image").expect("root text present");
    let reply_pos = body
        .find("itzg/minecraft-server")
        .expect("reply text present");
    assert!(root_pos < reply_pos, "messages ordered by ts");

    // HTML entity unescaped and link label rendered.
    assert!(
        body.contains("& the demo"),
        "&amp; unescaped, body:\n{body}"
    );

    // Header carries channel, date, participants, topic.
    assert!(body.contains("Channel: #craft"));
    assert!(body.contains("Topic: where the building happens"));
    assert!(
        body.contains("Date: 2025-01-01"),
        "root-date header, body:\n{body}"
    );

    // File indexed by name+type, sets has_files.
    assert!(
        body.contains("attached: diagram.png (PNG)"),
        "file rendered, body:\n{body}"
    );
    assert_eq!(thread.meta_json["has_files"], true);

    // Reactions rendered.
    assert!(
        body.contains("reactions: eyes×2"),
        "reactions rendered, body:\n{body}"
    );

    // source == slack and content_hash == hash_body(body) == meta content_hash.
    assert_eq!(thread.meta_json["source"], "slack");
    assert_eq!(thread.content_hash, source_meta::hash_body(&thread.body));
    assert_eq!(thread.meta_json["content_hash"], thread.content_hash);

    // Authors metadata holds both resolved names.
    let authors = thread.meta_json["authors"]
        .as_array()
        .expect("authors array");
    let names: Vec<&str> = authors.iter().filter_map(|value| value.as_str()).collect();
    assert!(names.contains(&"ada"));
    assert!(names.contains(&"Bob Builder"));

    // A two-human thread is not a bot thread, and craft is not external/archived.
    assert_eq!(thread.meta_json["is_bot_thread"], false);
    assert_eq!(thread.meta_json["is_external"], false);
    assert_eq!(thread.meta_json["is_archived"], false);
    assert_eq!(thread.meta_json["channel_id"], "C0SYNTH001");
    assert_eq!(thread.meta_json["channel_name"], "craft");

    // timestamp is the root ts integer part, epoch seconds.
    assert_eq!(thread.meta_json["timestamp"], 1_735_689_700_i64);
}

/// The standalone message becomes its own single-message thread keyed on its ts.
#[test]
fn standalone_message_is_its_own_thread() {
    let documents = collect_documents();
    let index = by_external_id(&documents);

    let standalone = index
        .get("slack:C0SYNTH001:1735689900.000300")
        .expect("standalone document present");
    assert_eq!(standalone.meta_json["message_count"], 1);

    let body = String::from_utf8(standalone.body.clone()).expect("utf-8");
    // Link label rendered, channel ref rendered, no raw markup tokens.
    assert!(body.contains("see the docs"), "link label, body:\n{body}");
    assert!(body.contains("#old-stuff"), "channel ref, body:\n{body}");
    assert!(!body.contains("<#"), "no raw channel tokens, body:\n{body}");
}

/// The bot message (no top-level text) becomes a bot thread, pulling prose from
/// its attachment and marking the author with `(bot)`.
#[test]
fn bot_message_marked_and_prose_from_attachment() {
    let documents = collect_documents();
    let index = by_external_id(&documents);

    let bot = index
        .get("slack:C0SYNTH001:1735690000.000400")
        .expect("bot document present");
    assert_eq!(bot.meta_json["is_bot_thread"], true);

    let body = String::from_utf8(bot.body.clone()).expect("utf-8");
    assert!(
        body.contains("[Better Stack (bot)]"),
        "bot author labelled, body:\n{body}"
    );
    assert!(
        body.contains("Monitor *ix.dev* recovered"),
        "attachment prose, body:\n{body}"
    );
}

/// `channel_join` / `channel_leave` system messages are never emitted.
#[test]
fn join_and_leave_messages_dropped() {
    let documents = collect_documents();
    let index = by_external_id(&documents);

    // Join/leave ts values must not appear as their own documents.
    assert!(
        !index.contains_key("slack:C0SYNTH001:1735689600.000100"),
        "join not a document"
    );
    assert!(
        !index.contains_key("slack:C0SYNTH001:1735776100.000600"),
        "leave not a document"
    );

    for doc in &documents {
        let body = String::from_utf8(doc.body.clone()).expect("utf-8");
        assert!(
            !body.contains("has joined the channel"),
            "no join text in {}",
            doc.external_id
        );
        assert!(
            !body.contains("has left the channel"),
            "no leave text in {}",
            doc.external_id
        );
    }

    // craft has exactly three real threads: cross-file thread, standalone, bot.
    let craft = documents
        .iter()
        .filter(|doc| doc.meta_json["channel_id"] == "C0SYNTH001")
        .count();
    assert_eq!(craft, 3, "join/leave dropped; 3 content threads remain");
}

/// Re-running the adapter over an unchanged export yields byte-identical bodies
/// and content hashes (deterministic, so re-ingest is a no-op).
#[test]
fn output_is_deterministic_across_runs() {
    let first = collect_documents();
    let second = collect_documents();
    assert_eq!(first.len(), second.len());

    let first_index = by_external_id(&first);
    for doc in &second {
        let prior = first_index
            .get(doc.external_id.as_str())
            .expect("same ids on re-run");
        assert_eq!(
            prior.content_hash, doc.content_hash,
            "stable hash for {}",
            doc.external_id
        );
        assert_eq!(prior.body, doc.body, "stable body for {}", doc.external_id);
    }
}

/// An archived channel surfaces its archived flag in both body and metadata,
/// even when it has only system messages and so produces no documents — here we
/// just confirm the empty archived channel does not crash iteration.
#[test]
fn empty_channel_produces_no_documents_without_error() {
    // old-stuff (C0SYNTH002) has no day files in the fixture, so it contributes
    // nothing; the iteration must still succeed and ignore it.
    let documents = collect_documents();
    let from_old = documents
        .iter()
        .any(|doc| doc.meta_json["channel_id"] == "C0SYNTH002");
    assert!(!from_old, "empty channel yields nothing");
}
