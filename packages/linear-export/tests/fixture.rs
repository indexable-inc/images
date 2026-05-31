//! Fixture-driven tests for the Linear export adapter.
//!
//! These run entirely against the synthetic, anonymized fixture under
//! `tests/fixture/` and never touch the private production export.
#![expect(
    clippy::expect_used,
    reason = "Test code: a failed expectation is a test failure"
)]

use std::path::PathBuf;

use linear_export::LinearExport;
use search_meta::{Document, Source, SourceAdapter, hash_body};
use serde_json::Value;

/// Path to the synthetic fixture directory shipped with this crate.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

/// Open the fixture and collect every document, failing the test on any error.
fn collect_docs() -> Vec<Document> {
    let export = LinearExport::open(&fixture_dir()).expect("open fixture export");
    export
        .documents()
        .collect::<Result<Vec<_>, _>>()
        .expect("project all fixture issues into documents")
}

/// Read a metadata key as a JSON value, asserting it is present.
fn meta<'a>(doc: &'a Document, key: &str) -> &'a Value {
    doc.meta_json.get(key).expect("metadata key present")
}

/// Read a metadata key as a string, asserting presence and type.
fn meta_str<'a>(doc: &'a Document, key: &str) -> &'a str {
    meta(doc, key).as_str().expect("metadata value is a string")
}

/// The body decoded as UTF-8 text.
fn body_text(doc: &Document) -> String {
    String::from_utf8(doc.body.clone()).expect("utf8 body")
}

/// Look up a document by its issue identifier in the metadata.
fn doc_for(docs: &[Document], identifier: &str) -> Document {
    docs.iter()
        .find(|d| d.meta_json.get("identifier").and_then(Value::as_str) == Some(identifier))
        .cloned()
        .expect("a document for the requested identifier")
}

/// The fixture has exactly four issues, so the adapter yields four documents.
#[test]
fn yields_one_document_per_issue() {
    let docs = collect_docs();
    assert_eq!(docs.len(), 4, "one document per fixture issue");
}

/// The adapter reports the Linear source and the team key from `metadata.json`.
#[test]
fn reports_source_and_team_key() {
    let export = LinearExport::open(&fixture_dir()).expect("open fixture export");
    assert_eq!(export.source(), Source::new("linear"));
    assert_eq!(export.team_key(), "FIX");
    assert_eq!(export.len(), 4);
    assert!(!export.is_empty());
}

/// Every document's identity and common-envelope metadata are well formed.
#[test]
fn documents_have_stable_identity_and_envelope() {
    for doc in collect_docs() {
        assert!(
            doc.external_id.starts_with("linear:issue:"),
            "external_id is namespaced: {}",
            doc.external_id
        );
        assert_eq!(doc.mime, "text/plain");

        assert!(doc.meta_json.is_object(), "meta_json is a flat object");
        assert_eq!(meta_str(&doc, "source"), "linear");
        assert_eq!(meta_str(&doc, "external_id"), doc.external_id);
        assert_eq!(meta_str(&doc, "content_hash"), doc.content_hash);
        assert!(
            doc.meta_json.get("state_type").is_some(),
            "state_type present"
        );
        assert!(doc.meta_json.get("title").is_some());

        // content_hash is the hash of the exact embedded bytes.
        assert_eq!(doc.content_hash, hash_body(&doc.body));
    }
}

/// FIX-1 links a `/pull/` attachment, so `has_pr` is true and `pr_urls` lists it.
#[test]
fn detects_pull_request_attachments() {
    let docs = collect_docs();
    let fix1 = doc_for(&docs, "FIX-1");
    assert_eq!(meta(&fix1, "has_pr"), &Value::Bool(true));
    let prs = meta(&fix1, "pr_urls").as_array().expect("pr_urls array");
    assert_eq!(prs.len(), 1, "only the /pull/ url counts");
    assert_eq!(
        prs.first().and_then(Value::as_str),
        Some("https://github.com/acme/repo/pull/42")
    );

    // A non-PR issue has has_pr=false and an empty pr_urls list.
    let fix2 = doc_for(&docs, "FIX-2");
    assert_eq!(meta(&fix2, "has_pr"), &Value::Bool(false));
    assert!(meta(&fix2, "pr_urls").as_array().expect("array").is_empty());
}

/// Comments are rendered oldest-first even though the export is newest-first.
#[test]
fn comments_are_sorted_ascending() {
    let docs = collect_docs();
    let fix1 = doc_for(&docs, "FIX-1");
    let body = body_text(&fix1);

    assert!(body.contains("Comments (2):"), "comment count line present");
    let first = body
        .find("First comment in time")
        .expect("first comment present");
    let second = body
        .find("Second comment in time")
        .expect("second comment present");
    assert!(
        first < second,
        "the chronologically earlier comment renders before the later one"
    );
    // A null comment author renders as "unknown" rather than panicking.
    assert!(body.contains("[unknown @ "), "null comment user handled");
}

/// The archived issue is still indexed and flagged `is_archived`.
#[test]
fn archived_issue_is_indexed_and_flagged() {
    let docs = collect_docs();
    let fix2 = doc_for(&docs, "FIX-2");
    assert_eq!(meta(&fix2, "is_archived"), &Value::Bool(true));
    assert_eq!(meta_str(&fix2, "state_type"), "completed");
    assert_eq!(meta_str(&fix2, "parent_identifier"), "FIX-1");

    // Non-archived issues are flagged false.
    let fix1 = doc_for(&docs, "FIX-1");
    assert_eq!(meta(&fix1, "is_archived"), &Value::Bool(false));
}

/// A null assignee omits `assignee_email` and renders "unassigned" in the body.
#[test]
fn null_assignee_is_handled() {
    let docs = collect_docs();
    let fix3 = doc_for(&docs, "FIX-3");
    assert!(
        fix3.meta_json.get("assignee_email").is_none(),
        "assignee_email omitted when there is no assignee"
    );
    assert!(body_text(&fix3).contains("Assignee: unassigned"));

    // An assigned issue carries the email.
    let fix1 = doc_for(&docs, "FIX-1");
    assert_eq!(meta_str(&fix1, "assignee_email"), "alex@acme.test");
}

/// A null description renders the placeholder but the issue is still indexed.
#[test]
fn null_description_uses_placeholder() {
    let docs = collect_docs();
    let fix4 = doc_for(&docs, "FIX-4");
    let body = body_text(&fix4);
    assert!(body.contains("Description:\n(no description)"));
    // A null creator renders as "unknown" rather than panicking.
    assert!(body.contains("Created by unknown on "));
}

/// Re-running the adapter over the same export yields identical content hashes.
#[test]
fn content_hash_is_deterministic() {
    let first = collect_docs();
    let second = collect_docs();
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(second.iter()) {
        assert_eq!(a.external_id, b.external_id);
        assert_eq!(a.content_hash, b.content_hash, "stable hash across runs");
    }
}
