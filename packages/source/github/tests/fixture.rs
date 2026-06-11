//! Fixture-driven tests for the GitHub export adapter.
//!
//! These run entirely against the synthetic fixture under `tests/fixture/` and
//! never touch a real GitHub export.
#![expect(
    clippy::expect_used,
    reason = "Test code: a failed expectation is a test failure"
)]

use std::path::PathBuf;

use serde_json::Value;
use source_github::GithubExport;
use source_meta::{Document, Source, SourceAdapter, hash_body};

/// Path to the synthetic fixture directory shipped with this crate.
fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

/// Open the fixture and collect every document, failing the test on any error.
fn collect_docs() -> Vec<Document> {
    let export = GithubExport::open(&fixture_dir()).expect("open fixture export");
    export
        .documents()
        .collect::<Result<Vec<_>, _>>()
        .expect("project all fixture items into documents")
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

/// Look up a document by its `external_id`.
fn doc_for(docs: &[Document], external_id: &str) -> Document {
    docs.iter()
        .find(|d| d.external_id == external_id)
        .cloned()
        .unwrap_or_else(|| panic!("a document for {external_id}"))
}

/// The fixture has four items, so the adapter yields four documents.
#[test]
fn yields_one_document_per_item() {
    let docs = collect_docs();
    assert_eq!(docs.len(), 4, "one document per fixture item");
}

/// The adapter reports the GitHub source and the repos from `metadata.json`.
#[test]
fn reports_source_and_repos() {
    let export = GithubExport::open(&fixture_dir()).expect("open fixture export");
    assert_eq!(export.source(), Source::new("github"));
    assert_eq!(export.repos(), ["acme/widgets", "acme/gadgets"]);
    assert_eq!(export.len(), 4);
    assert!(!export.is_empty());
}

/// Every document's identity and common-envelope metadata are well formed.
#[test]
fn documents_have_stable_identity_and_envelope() {
    for doc in collect_docs() {
        assert!(
            doc.external_id.starts_with("github:"),
            "external_id is namespaced: {}",
            doc.external_id
        );
        assert_eq!(doc.mime, "text/plain");

        assert!(doc.meta_json.is_object(), "meta_json is a flat object");
        assert_eq!(meta_str(&doc, "source"), "github");
        assert_eq!(meta_str(&doc, "external_id"), doc.external_id);
        assert_eq!(meta_str(&doc, "content_hash"), doc.content_hash);
        assert!(doc.meta_json.get("repo").is_some(), "repo present");
        assert!(doc.meta_json.get("number").is_some(), "number present");
        assert!(doc.meta_json.get("state").is_some(), "state present");
        assert!(doc.meta_json.get("title").is_some());

        // content_hash is the hash of the exact embedded bytes.
        assert_eq!(doc.content_hash, hash_body(&doc.body));
    }
}

/// `external_id` is `github:<owner>/<repo>:<number>` and spans repos.
#[test]
fn external_id_namespaces_repo_and_number() {
    let docs = collect_docs();
    let issue = doc_for(&docs, "github:acme/widgets:1");
    assert_eq!(meta_str(&issue, "repo"), "acme/widgets");
    assert_eq!(meta(&issue, "number"), &Value::from(1));

    // A second repo is represented in the same export.
    let pr = doc_for(&docs, "github:acme/gadgets:7");
    assert_eq!(meta_str(&pr, "repo"), "acme/gadgets");
}

/// Issues and pull requests are distinguished by `is_pr`, and PRs carry draft.
#[test]
fn separates_issues_and_pull_requests() {
    let docs = collect_docs();
    let issue = doc_for(&docs, "github:acme/widgets:1");
    assert_eq!(meta(&issue, "is_pr"), &Value::Bool(false));
    assert!(
        issue.meta_json.get("is_draft").is_none(),
        "issues carry no is_draft key"
    );

    let pr = doc_for(&docs, "github:acme/gadgets:7");
    assert_eq!(meta(&pr, "is_pr"), &Value::Bool(true));
    assert_eq!(meta(&pr, "is_draft"), &Value::Bool(false));

    let draft = doc_for(&docs, "github:acme/gadgets:8");
    assert_eq!(meta(&draft, "is_draft"), &Value::Bool(true));
    assert!(body_text(&draft).contains("(draft)"));
}

/// A merged PR keeps the `merged` state and renders the merge line.
#[test]
fn merged_pull_request_is_flagged() {
    let docs = collect_docs();
    let pr = doc_for(&docs, "github:acme/gadgets:7");
    assert_eq!(meta_str(&pr, "state"), "merged");
    let body = body_text(&pr);
    assert!(body.contains("State: merged"));
    assert!(body.contains("Merged 2026-01-08T09:00:00Z"));
    assert!(body.contains("Branches: feat/retry -> main"));
}

/// Comments render oldest-first even when the export lists them newest-first.
#[test]
fn comments_are_sorted_ascending() {
    let docs = collect_docs();
    let issue = doc_for(&docs, "github:acme/widgets:1");
    let body = body_text(&issue);

    assert!(body.contains("Comments (2):"), "comment count line present");
    let first = body.find("First comment in time").expect("first comment");
    let second = body.find("Second comment in time").expect("second comment");
    assert!(first < second, "earlier comment renders before later one");
    // A null comment author renders as "unknown" rather than panicking.
    assert!(body.contains("[unknown @ "), "null comment author handled");
}

/// PR reviews and inline review threads render, with diff location and ordering.
#[test]
fn renders_reviews_and_review_threads() {
    let docs = collect_docs();
    let pr = doc_for(&docs, "github:acme/gadgets:7");
    let body = body_text(&pr);

    assert!(body.contains("Reviews (1):"));
    assert!(body.contains("APPROVED"));
    assert!(body.contains("Looks good to me."));

    assert!(body.contains("Review threads (1):"));
    assert!(body.contains("[src/lib.rs:10]"));
    // Thread comments are oldest-first, and a null thread author is "unknown".
    let cap = body
        .find("Cap the backoff here.")
        .expect("first thread comment");
    let fixed = body
        .find("Good catch, fixed.")
        .expect("second thread comment");
    assert!(cap < fixed, "thread comments render oldest-first");
}

/// An empty body renders the placeholder; the item is still indexed.
#[test]
fn empty_body_uses_placeholder() {
    let docs = collect_docs();
    let issue = doc_for(&docs, "github:acme/widgets:2");
    let body = body_text(&issue);
    assert!(body.contains("Body:\n(no body)"));
    assert!(body.contains("Closed 2026-01-05T09:00:00Z"));
    // A null author renders as "unknown".
    assert!(body.contains("Author: unknown"));
    // No author means no author_name key.
    assert!(issue.meta_json.get("author_name").is_none());
}

/// An assigned, authored item carries `author_name` and assignees metadata.
#[test]
fn author_and_assignees_metadata() {
    let docs = collect_docs();
    let issue = doc_for(&docs, "github:acme/widgets:1");
    assert_eq!(meta_str(&issue, "author_name"), "alex");
    let assignees = meta(&issue, "assignees")
        .as_array()
        .expect("assignees array");
    assert_eq!(assignees.len(), 1);
    assert_eq!(assignees.first().and_then(Value::as_str), Some("alex"));
    let labels = meta(&issue, "labels").as_array().expect("labels array");
    assert_eq!(labels.first().and_then(Value::as_str), Some("bug"));
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
