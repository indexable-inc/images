//! Build a temporary atuin-shaped sqlite db and check the projected documents.

use rusqlite::{Connection, params};
use source_atuin::AtuinHistory;
use source_meta::{Source, SourceAdapter as _};

/// Create an atuin-schema history db at `path` with a few rows: two live
/// commands, one soft-deleted, and one empty (both of which must be skipped).
fn make_db(path: &std::path::Path) {
    let conn = Connection::open(path).expect("open");
    conn.execute_batch(
        "create table history (
            id text primary key, timestamp integer, duration integer, exit integer,
            command text, cwd text, session text, hostname text, deleted_at integer,
            author text, intent text
        );",
    )
    .expect("schema");
    let insert = |id: &str, ts: i64, exit: i64, cmd: &str, deleted: Option<i64>| {
        conn.execute(
            "insert into history (id, timestamp, exit, command, cwd, session, hostname, deleted_at) \
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, ts, exit, cmd, "/home/x/proj", "sess1", "host1:andrew", deleted],
        )
        .expect("insert");
    };
    insert("id1", 1_773_725_019_000_000_000, 0, "git status", None);
    insert("id2", 1_773_725_120_000_000_000, 1, "cargo build", None);
    insert("id3", 1_773_725_200_000_000_000, 0, "secret", Some(1_773_725_300_000_000_000));
    insert("id4", 1_773_725_400_000_000_000, 0, "   ", None);
}

// The repo compiles tests with bare `rustc --test` (not `cargo test`), so the
// cargo-only `CARGO_TARGET_TMPDIR` is absent; use a runtime temp dir, unique per
// test so parallel test processes never share a db file.
fn open_fixture(tag: &str) -> AtuinHistory {
    let path = std::env::temp_dir().join(format!("source-atuin-test-{tag}.db"));
    let _ = std::fs::remove_file(&path);
    make_db(&path);
    AtuinHistory::open(&path).expect("open atuin db")
}

#[test]
fn skips_deleted_and_empty_commands() {
    let history = open_fixture("skips");
    // id3 is soft-deleted, id4 is blank: only id1 and id2 remain.
    assert_eq!(history.len(), 2);
    assert!(!history.is_empty());
}

#[test]
fn uninitialized_db_is_a_typed_skip() {
    // atuin creates the db file before its first-run migration adds the
    // `history` table, so a freshly-seen account can have a db with no tables.
    // Opening it must yield the typed `UninitializedDb` skip (so the fleet
    // indexer treats it as a soft, non-fatal skip), not a generic query error.
    let path = std::env::temp_dir().join("source-atuin-test-uninit.db");
    let _ = std::fs::remove_file(&path);
    // Create an empty db (a connection with no schema), matching atuin's
    // pre-migration state.
    Connection::open(&path).expect("create empty db");

    let error = AtuinHistory::open(&path).expect_err("uninitialized db must error");
    assert!(error.is_uninitialized(), "expected UninitializedDb, got {error:?}");
    assert!(
        error.to_string().contains("uninitialized"),
        "skip message should explain the db is uninitialized: {error}"
    );
}

#[test]
fn corrupt_db_is_not_a_skip() {
    // A genuine failure (a file that is not a valid sqlite db) must remain a
    // hard error so real-error reporting is preserved; only the uninitialized
    // case is downgraded to a skip.
    let path = std::env::temp_dir().join("source-atuin-test-corrupt.db");
    std::fs::write(&path, b"this is not a sqlite database").expect("write garbage");

    let error = AtuinHistory::open(&path).expect_err("corrupt db must error");
    assert!(!error.is_uninitialized(), "a corrupt db must not be treated as a soft skip: {error:?}");
}

#[test]
fn documents_carry_shell_source_and_tags() {
    let history = open_fixture("tags");
    assert_eq!(history.source(), Source::new("shell"));

    let docs: Vec<_> = history.documents().map(|doc| doc.expect("document")).collect();
    let first = &docs[0];
    assert_eq!(first.external_id, "atuin:id1");
    assert_eq!(first.meta_json["source"], "shell");
    assert_eq!(first.meta_json["host"], "host1");
    assert_eq!(first.meta_json["user"], "andrew");
    assert_eq!(first.meta_json["cwd"], "/home/x/proj");
    assert_eq!(first.meta_json["session_id"], "sess1");
    assert_eq!(first.meta_json["exit_status"], 0);
    // atuin nanoseconds are folded to epoch seconds.
    assert_eq!(first.meta_json["timestamp"], 1_773_725_019_i64);
    assert_eq!(first.content_hash, first.meta_json["content_hash"]);
    assert_eq!(first.body, b"git status");
}
