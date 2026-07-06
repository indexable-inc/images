//! Integration tests for the merge engine.
//!
//! Each test builds base/ours/theirs `SQLite` fixtures in a tempdir, runs the
//! merge, and asserts either a clean merge (with the expected row state) or a
//! specific typed refusal.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use sqlmerge::{MergeError, PolicyConfig, merge};
use tempfile::TempDir;

/// Parse a `sqlmerge.toml` body into a [`PolicyConfig`] for a test.
fn policies(toml: &str) -> PolicyConfig {
    PolicyConfig::load_body(toml).expect("valid test config")
}

/// A base/ours/theirs fixture triple living in one tempdir.
struct Fixture {
    _dir: TempDir,
    base: PathBuf,
    ours: PathBuf,
    theirs: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("tempdir");
        let base = dir.path().join("base.db");
        let ours = dir.path().join("ours.db");
        let theirs = dir.path().join("theirs.db");
        Self {
            _dir: dir,
            base,
            ours,
            theirs,
        }
    }

    /// Run the merge under the all-abort default (no `sqlmerge.toml`).
    fn run(&self) -> sqlmerge::Result<()> {
        self.run_with(&PolicyConfig::abort_all())
    }

    /// Run the merge under an explicit policy config.
    fn run_with(&self, policies: &PolicyConfig) -> sqlmerge::Result<()> {
        merge(
            path(&self.base),
            path(&self.ours),
            path(&self.theirs),
            policies,
        )
    }
}

fn path(p: &Path) -> &str {
    p.to_str().expect("utf8 path")
}

/// Open a db, run a batch of SQL, close it.
fn build(db: &Path, sql: &str) {
    let conn = Connection::open(db).expect("open");
    conn.execute_batch(sql).expect("exec batch");
}

/// Copy `from` to `to` as an independent byte-copy of the base database.
/// `build` opens each fixture with its own connection and closes it before we
/// copy, so a plain filesystem copy is safe (no open WAL to reconcile).
fn copy_db(from: &Path, to: &Path) {
    std::fs::copy(from, to).expect("copy db");
}

/// Read a single integer cell.
fn read_int(db: &Path, sql: &str) -> i64 {
    let conn = Connection::open(db).expect("open");
    conn.query_row(sql, [], |r| r.get(0)).expect("query")
}

/// Read a single text cell.
fn read_text(db: &Path, sql: &str) -> String {
    let conn = Connection::open(db).expect("open");
    conn.query_row(sql, [], |r| r.get(0)).expect("query")
}

const USERS_SCHEMA: &str =
    "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score INTEGER NOT NULL);";

/// A standard base with three users; ours and theirs start as copies.
fn seeded_users() -> Fixture {
    let f = Fixture::new();
    build(
        &f.base,
        &format!(
            "{USERS_SCHEMA}\n\
             INSERT INTO users VALUES (1, 'alice', 10), (2, 'bob', 20), (3, 'carol', 30);"
        ),
    );
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);
    f
}

#[test]
fn non_overlapping_row_edits_merge_clean() {
    let f = seeded_users();
    // ours edits alice; theirs edits bob. Disjoint rows.
    build(&f.ours, "UPDATE users SET score = 11 WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 22 WHERE id = 2;");

    f.run().expect("clean merge");

    // ours keeps its alice edit and gains theirs's bob edit.
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 11);
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 2"), 22);
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 3"), 30);
}

#[test]
fn same_cell_divergent_edit_conflicts() {
    let f = seeded_users();
    // Both edit alice's score to different values.
    build(&f.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    let err = f.run().expect_err("should conflict");
    let MergeError::Conflicts(conflicts) = err else {
        panic!("expected Conflicts, got {err:?}");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].table, "users");

    // ours must be left unchanged (aborted apply, no partial write).
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 111);
}

#[test]
fn inserts_with_different_pks_merge() {
    let f = seeded_users();
    build(&f.ours, "INSERT INTO users VALUES (4, 'dave', 40);");
    build(&f.theirs, "INSERT INTO users VALUES (5, 'erin', 50);");

    f.run().expect("clean merge");

    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users"), 5);
    assert_eq!(read_text(&f.ours, "SELECT name FROM users WHERE id = 4"), "dave");
    assert_eq!(read_text(&f.ours, "SELECT name FROM users WHERE id = 5"), "erin");
}

#[test]
fn identical_insert_both_sides_is_not_a_conflict() {
    let f = seeded_users();
    // Same PK, same values, on both sides.
    build(&f.ours, "INSERT INTO users VALUES (6, 'frank', 60);");
    build(&f.theirs, "INSERT INTO users VALUES (6, 'frank', 60);");

    f.run().expect("identical insert should merge clean");

    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users WHERE id = 6"), 1);
    assert_eq!(read_text(&f.ours, "SELECT name FROM users WHERE id = 6"), "frank");
}

#[test]
fn convergent_delete_merges_clean() {
    let f = seeded_users();
    // Both sides delete carol. The changeset's DELETE finds the row already
    // gone (NOTFOUND) and the end state matches on both sides: not a conflict.
    build(&f.ours, "DELETE FROM users WHERE id = 3;");
    build(&f.theirs, "DELETE FROM users WHERE id = 3;");

    f.run().expect("convergent delete should merge clean");

    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users WHERE id = 3"), 0);
    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users"), 2);
}

#[test]
fn delete_vs_update_conflicts() {
    let f = seeded_users();
    // theirs deletes alice; ours edited her. The changeset DELETE finds a row
    // whose values differ from base (DATA conflict): a real conflict.
    build(&f.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f.theirs, "DELETE FROM users WHERE id = 1;");

    let err = f.run().expect_err("delete-vs-update should conflict");
    let MergeError::Conflicts(conflicts) = err else {
        panic!("expected Conflicts, got {err:?}");
    };
    assert_eq!(conflicts[0].table, "users");

    // ours untouched by the aborted apply.
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 111);
}

#[test]
fn update_vs_delete_conflicts() {
    let f = seeded_users();
    // theirs edits alice; ours deleted her. The changeset UPDATE finds no row
    // (NOTFOUND on update): a real conflict, unlike the convergent delete.
    build(&f.ours, "DELETE FROM users WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    let err = f.run().expect_err("update-vs-delete should conflict");
    let MergeError::Conflicts(conflicts) = err else {
        panic!("expected Conflicts, got {err:?}");
    };
    assert_eq!(conflicts[0].table, "users");

    // ours untouched by the aborted apply: alice stays deleted.
    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users WHERE id = 1"), 0);
}

#[test]
fn base_schema_divergence_refused() {
    let f = seeded_users();
    // Both sides apply the SAME migration, so ours == theirs, but base is
    // behind. The session diff cannot span a schema change; refuse typed.
    build(&f.ours, "ALTER TABLE users ADD COLUMN email TEXT;");
    build(&f.theirs, "ALTER TABLE users ADD COLUMN email TEXT;");

    let err = f.run().expect_err("should refuse");
    let MergeError::BaseSchemaDiverged(objects) = err else {
        panic!("expected BaseSchemaDiverged, got {err:?}");
    };
    assert_eq!(objects, vec!["users".to_string()]);
}

#[test]
fn missing_primary_key_table_refused() {
    let f = Fixture::new();
    // `logs` has no PRIMARY KEY, so the session extension would silently skip it.
    build(
        &f.base,
        "CREATE TABLE logs (msg TEXT NOT NULL, at INTEGER NOT NULL);",
    );
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);
    build(&f.theirs, "INSERT INTO logs VALUES ('hi', 1);");

    let err = f.run().expect_err("should refuse");
    let MergeError::MissingPrimaryKey(tables) = err else {
        panic!("expected MissingPrimaryKey, got {err:?}");
    };
    assert_eq!(tables, vec!["logs".to_string()]);
}

#[test]
fn schema_divergence_refused() {
    let f = Fixture::new();
    build(&f.base, USERS_SCHEMA);
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);
    // theirs adds a column: DDL divergence.
    build(&f.theirs, "ALTER TABLE users ADD COLUMN email TEXT;");

    let err = f.run().expect_err("should refuse");
    let MergeError::SchemaDiverged(objs) = err else {
        panic!("expected SchemaDiverged, got {err:?}");
    };
    assert!(objs.iter().any(|o| o.object == "users"));
}

#[test]
fn schema_whitespace_difference_is_not_divergence() {
    let f = Fixture::new();
    build(
        &f.base,
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v INTEGER NOT NULL);\n\
         INSERT INTO t VALUES (1, 1);",
    );
    copy_db(&f.base, &f.ours);
    // Rebuild theirs from scratch with reformatted (but equivalent) DDL.
    build(
        &f.theirs,
        "CREATE TABLE t (\n  id  INTEGER  PRIMARY KEY,\n  v   INTEGER  NOT NULL\n);\n\
         INSERT INTO t VALUES (1, 1);",
    );
    build(&f.theirs, "UPDATE t SET v = 2 WHERE id = 1;");

    f.run().expect("whitespace-only schema difference should merge");
    assert_eq!(read_int(&f.ours, "SELECT v FROM t WHERE id = 1"), 2);
}

const FK_SCHEMA: &str = "CREATE TABLE parent (id INTEGER PRIMARY KEY);\n\
     CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER NOT NULL \
     REFERENCES parent(id));";

#[test]
fn foreign_key_violation_from_merge_caught() {
    let f = Fixture::new();
    build(
        &f.base,
        &format!("{FK_SCHEMA}\nINSERT INTO parent VALUES (1);\nINSERT INTO child VALUES (10, 1);"),
    );
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);

    // ours deletes the parent that theirs's new child will reference.
    build(&f.ours, "PRAGMA foreign_keys=OFF; DELETE FROM parent WHERE id = 1;");
    // theirs adds a child pointing at parent 1 (still present in theirs).
    build(&f.theirs, "INSERT INTO child VALUES (11, 1);");

    // Applying theirs's new child (parent_id=1) onto ours (parent 1 gone) is an
    // FK violation. It must be caught and refused (exit 1). SQLite's changeset
    // apply raises this as a FOREIGN_KEY conflict; either that or the
    // post-merge PRAGMA foreign_key_check is an acceptable catch, as long as we
    // refuse.
    let err = f.run().expect_err("should catch FK violation");
    assert!(
        matches!(
            err,
            MergeError::Conflicts(_) | MergeError::ForeignKeyCheckFailed(_)
        ),
        "expected an FK-related refusal, got {err:?}"
    );
}

#[test]
fn preexisting_dangling_fk_caught_by_post_merge_check() {
    let f = Fixture::new();
    build(
        &f.base,
        &format!("{FK_SCHEMA}\nINSERT INTO parent VALUES (1);\nINSERT INTO child VALUES (10, 1);"),
    );
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);

    // ours already holds a dangling child (parent 2 never existed), created
    // with FK enforcement off. The base->theirs changeset only touches parent,
    // so the apply does not re-check this row; the post-merge
    // foreign_key_check must.
    build(
        &f.ours,
        "PRAGMA foreign_keys=OFF; INSERT INTO child VALUES (99, 2);",
    );
    build(&f.theirs, "INSERT INTO parent VALUES (3);");

    let err = f.run().expect_err("should catch pre-existing dangling FK");
    let MergeError::ForeignKeyCheckFailed(rows) = err else {
        panic!("expected ForeignKeyCheckFailed, got {err:?}");
    };
    assert!(!rows.is_empty());
}

// --- per-table conflict policies (sqlmerge.toml) ---------------------------

/// The same-cell divergent edit that aborts by default is resolved to ours
/// under the `ours` policy: the incoming change is omitted, ours is kept.
#[test]
fn ours_policy_keeps_our_value_on_data_conflict() {
    let f = seeded_users();
    build(&f.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    f.run_with(&policies("[policies]\n\"users\" = \"ours\"\n"))
        .expect("ours policy resolves the conflict");

    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 111);
}

/// Under `theirs`, a DATA conflict (both edited the same row) is `REPLACEd`
/// with theirs's value.
#[test]
fn theirs_policy_takes_their_value_on_data_conflict() {
    let f = seeded_users();
    build(&f.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    f.run_with(&policies("[policies]\n\"users\" = \"theirs\"\n"))
        .expect("theirs policy resolves the conflict");

    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 222);
}

/// Under `theirs`, a CONFLICT-type row (both inserted the same PK with
/// different values) is `REPLACEd` with theirs.
#[test]
fn theirs_policy_replaces_conflicting_insert() {
    let f = seeded_users();
    build(&f.ours, "INSERT INTO users VALUES (7, 'ours', 70);");
    build(&f.theirs, "INSERT INTO users VALUES (7, 'theirs', 77);");

    f.run_with(&policies("[policies]\n\"users\" = \"theirs\"\n"))
        .expect("theirs policy resolves the insert conflict");

    assert_eq!(read_text(&f.ours, "SELECT name FROM users WHERE id = 7"), "theirs");
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 7"), 77);
}

/// REPLACE is illegal for a NOTFOUND conflict (theirs updated a row ours
/// deleted). Under `theirs`, that row must still abort rather than return an
/// illegal REPLACE that would fail the whole apply with `SQLITE_MISUSE`.
#[test]
fn theirs_policy_aborts_on_notfound_update() {
    let f = seeded_users();
    build(&f.ours, "DELETE FROM users WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    let err = f
        .run_with(&policies("[policies]\n\"users\" = \"theirs\"\n"))
        .expect_err("theirs cannot REPLACE a NOTFOUND row");
    let MergeError::Conflicts(conflicts) = err else {
        panic!("expected Conflicts, got {err:?}");
    };
    assert_eq!(conflicts[0].table, "users");
    // Aborted apply leaves ours untouched: alice stays deleted.
    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users WHERE id = 1"), 0);
}

/// Under `append-only`, a conflicting INSERT (same PK, different values) keeps
/// ours (the incoming insert is omitted).
#[test]
fn append_only_keeps_ours_on_conflicting_insert() {
    let f = seeded_users();
    build(&f.ours, "INSERT INTO users VALUES (8, 'ours', 80);");
    build(&f.theirs, "INSERT INTO users VALUES (8, 'theirs', 88);");

    f.run_with(&policies("[policies]\n\"users\" = \"append-only\"\n"))
        .expect("append-only omits the conflicting insert");

    assert_eq!(read_text(&f.ours, "SELECT name FROM users WHERE id = 8"), "ours");
    assert_eq!(read_int(&f.ours, "SELECT count(*) FROM users WHERE id = 8"), 1);
}

/// Under `append-only`, a conflicting UPDATE still aborts: only inserts win.
#[test]
fn append_only_aborts_on_update_conflict() {
    let f = seeded_users();
    build(&f.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    let err = f
        .run_with(&policies("[policies]\n\"users\" = \"append-only\"\n"))
        .expect_err("append-only aborts an update conflict");
    let MergeError::Conflicts(conflicts) = err else {
        panic!("expected Conflicts, got {err:?}");
    };
    assert_eq!(conflicts[0].table, "users");
    assert_eq!(read_int(&f.ours, "SELECT score FROM users WHERE id = 1"), 111);
}

/// A glob applies the policy to matching tables and leaves the rest on the
/// abort default: `cache_*` takes theirs, `users` (unmatched) still aborts.
#[test]
fn glob_scopes_policy_to_matching_tables() {
    let f = Fixture::new();
    build(
        &f.base,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, score INTEGER NOT NULL);\n\
         CREATE TABLE cache_hot (id INTEGER PRIMARY KEY, v INTEGER NOT NULL);\n\
         INSERT INTO users VALUES (1, 10);\n\
         INSERT INTO cache_hot VALUES (1, 100);",
    );
    copy_db(&f.base, &f.ours);
    copy_db(&f.base, &f.theirs);

    // A conflict in cache_hot (theirs wins) and a clean edit in users.
    build(&f.ours, "UPDATE cache_hot SET v = 111 WHERE id = 1;");
    build(&f.theirs, "UPDATE cache_hot SET v = 222 WHERE id = 1;");

    f.run_with(&policies("[policies]\n\"cache_*\" = \"theirs\"\n"))
        .expect("cache_* conflict resolves to theirs");

    assert_eq!(read_int(&f.ours, "SELECT v FROM cache_hot WHERE id = 1"), 222);

    // And a conflict in the unmatched `users` table still aborts.
    let f2 = Fixture::new();
    build(
        &f2.base,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, score INTEGER NOT NULL);\n\
         INSERT INTO users VALUES (1, 10);",
    );
    copy_db(&f2.base, &f2.ours);
    copy_db(&f2.base, &f2.theirs);
    build(&f2.ours, "UPDATE users SET score = 111 WHERE id = 1;");
    build(&f2.theirs, "UPDATE users SET score = 222 WHERE id = 1;");

    let err = f2
        .run_with(&policies("[policies]\n\"cache_*\" = \"theirs\"\n"))
        .expect_err("users is unmatched and still aborts");
    assert!(matches!(err, MergeError::Conflicts(_)), "got {err:?}");
}
