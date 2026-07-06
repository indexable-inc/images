//! The three-way merge engine.
//!
//! Strategy: the `SQLite` session extension can compute a *changeset* describing
//! how one database differs from another (`sqlite3session_diff`, exposed as
//! `Session::diff`). We compute the changeset `base -> theirs` and apply it to
//! `ours`. The result is `ours` with theirs's changes layered on top, which is
//! exactly a three-way merge keyed by primary key:
//!
//!   - a row theirs changed but ours didn't: applies cleanly;
//!   - a row both sides changed to the same value: applies cleanly;
//!   - a row both sides changed differently: `DATA` conflict, abort;
//!   - a row theirs inserted that ours also inserted with different values at
//!     the same PK: `CONFLICT`, abort.
//!
//! Conflicts are captured per-row and reported; the default policy aborts the
//! whole apply (no partial writes).

use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;
use rusqlite::hooks::Action;
use rusqlite::session::{ChangesetItem, ConflictAction, ConflictType, Session};
use rusqlite::types::ValueRef;

use crate::error::{MergeError, PrimaryKey, Result, RowConflict};
use crate::schema;

/// Conflict-resolution policy. v1 ships only [`ConflictPolicy::Abort`]; the
/// enum exists so per-table policies (last-writer-wins, append-only) can be
/// added later without reshaping the apply path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Any conflict aborts the entire merge (exit 1). No auto-resolution.
    #[default]
    Abort,
}

/// Render a [`ValueRef`] as a short display string for conflict reporting.
fn render_value(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).map_or_else(
            |_| format!("<{} bytes text>", bytes.len()),
            |s| format!("'{s}'"),
        ),
        ValueRef::Blob(bytes) => format!("<blob {} bytes>", bytes.len()),
    }
}

/// `SQLite`-value equality for conflict resolution: same type, same payload.
/// `Real` compares bitwise via `to_bits` so NaN-vs-NaN and -0.0 are handled
/// deterministically; two rows written identically compare equal.
fn values_equal(a: ValueRef<'_>, b: ValueRef<'_>) -> bool {
    match (a, b) {
        (ValueRef::Null, ValueRef::Null) => true,
        (ValueRef::Integer(x), ValueRef::Integer(y)) => x == y,
        (ValueRef::Real(x), ValueRef::Real(y)) => x.to_bits() == y.to_bits(),
        (ValueRef::Text(x), ValueRef::Text(y)) | (ValueRef::Blob(x), ValueRef::Blob(y)) => x == y,
        _ => false,
    }
}

/// True for a `NOTFOUND` DELETE: the changeset deletes a row that ours already
/// deleted. Both sides want the row gone and it is gone, so omitting the change
/// converges. A `NOTFOUND` on an UPDATE stays a real conflict: theirs edited a
/// row ours deleted, and there is no obviously right answer.
fn is_convergent_delete(kind: &ConflictType, item: &ChangesetItem) -> bool {
    *kind == ConflictType::SQLITE_CHANGESET_NOTFOUND
        && item
            .op()
            .is_ok_and(|op| matches!(op.code(), Action::SQLITE_DELETE))
}

/// True if a `CONFLICT`-type item (a required INSERT that hit an existing row)
/// is a benign duplicate: ours (the existing row, via `conflict`) equals theirs
/// (the incoming row, via `new_value`) in every column. The session extension
/// reports every insert-over-existing as a conflict without comparing values,
/// so "both sides inserted the same row" surfaces here and must not abort.
fn is_benign_duplicate_insert(kind: &ConflictType, item: &ChangesetItem) -> bool {
    if *kind != ConflictType::SQLITE_CHANGESET_CONFLICT {
        return false;
    }
    let Ok(op) = item.op() else {
        return false;
    };
    let Ok(ncols) = usize::try_from(op.number_of_columns()) else {
        return false;
    };
    if ncols == 0 {
        return false;
    }

    for col in 0..ncols {
        let (Ok(ours), Ok(theirs)) = (item.conflict(col), item.new_value(col)) else {
            return false;
        };
        if !values_equal(ours, theirs) {
            return false;
        }
    }
    true
}

/// Pull the primary-key column values out of a conflicting item for reporting.
///
/// `pk()` returns a per-column mask of which columns form the PK. For those
/// columns we read the value: on an UPDATE/DELETE the PK lives in `old`, on an
/// INSERT it lives in `new`. We try old first, then new, and fall back to a
/// placeholder so reporting never fails the merge on its own.
fn extract_primary_key(item: &ChangesetItem, action: Action, ncols: usize) -> PrimaryKey {
    let mask = item.pk().map(<[u8]>::to_vec).unwrap_or_default();
    let mut parts = Vec::new();
    for col in 0..ncols {
        let is_pk = mask.get(col).copied().unwrap_or(0) != 0;
        if !is_pk {
            continue;
        }
        let value = match action {
            Action::SQLITE_INSERT => item.new_value(col).ok(),
            _ => item
                .old_value(col)
                .ok()
                .or_else(|| item.new_value(col).ok()),
        };
        parts.push(value.map_or_else(|| "?".to_string(), render_value));
    }
    PrimaryKey(parts)
}

/// Ours-vs-theirs column values for a `DATA`/`CONFLICT` conflict row.
struct ConflictValues {
    /// The row currently in ours, via `ChangesetItem::conflict`.
    ours: Vec<String>,
    /// The incoming row from theirs, via `ChangesetItem::new_value`.
    theirs: Vec<String>,
}

fn conflict_values(item: &ChangesetItem, ncols: usize) -> ConflictValues {
    let mut ours = Vec::new();
    let mut theirs = Vec::new();
    for col in 0..ncols {
        if let Ok(v) = item.conflict(col) {
            ours.push(render_value(v));
        }
        if let Ok(v) = item.new_value(col) {
            theirs.push(render_value(v));
        }
    }
    ConflictValues { ours, theirs }
}

/// Build the per-row conflict report for a conflict-handler callback.
///
/// The set of valid [`ChangesetItem`] accessors depends on the conflict type
/// (`SQLite` session docs). `FOREIGN_KEY` has no current row or operation; only
/// `fk_conflicts()` is legal, and calling `op()`/`pk()`/value accessors on it
/// dereferences a null pointer and crashes. `conflict()` values exist only for
/// `DATA` and `CONFLICT`.
fn describe_conflict(kind: &ConflictType, item: &ChangesetItem) -> RowConflict {
    if *kind == ConflictType::SQLITE_CHANGESET_FOREIGN_KEY {
        let n = item.fk_conflicts().unwrap_or(0);
        return RowConflict {
            table: "<deferred foreign key>".to_string(),
            kind: format!("{kind:?}"),
            primary_key: PrimaryKey(Vec::new()),
            ours: Vec::new(),
            theirs: vec![format!("{n} deferred FK violation(s)")],
        };
    }

    // A negative column count cannot occur (it is a length from SQLite); fold
    // that impossible case into the op-unavailable fallback rather than
    // panicking inside the C callback, where unwinding would abort.
    let (table, action, ncols) = item
        .op()
        .ok()
        .and_then(|o| {
            usize::try_from(o.number_of_columns())
                .ok()
                .map(|ncols| (o.table_name().to_string(), o.code(), ncols))
        })
        .unwrap_or_else(|| ("<unknown>".to_string(), Action::UNKNOWN, 0));

    let primary_key = extract_primary_key(item, action, ncols);
    let values = match kind {
        ConflictType::SQLITE_CHANGESET_DATA | ConflictType::SQLITE_CHANGESET_CONFLICT => {
            conflict_values(item, ncols)
        }
        _ => ConflictValues {
            ours: Vec::new(),
            theirs: Vec::new(),
        },
    };

    RowConflict {
        table,
        kind: format!("{kind:?}"),
        primary_key,
        ours: values.ours,
        theirs: values.theirs,
    }
}

/// Compute the `base -> theirs` changeset and apply it onto `ours` in place.
///
/// `ours_path` is opened read-write; on a clean merge it holds the merged
/// result. `sqlite3changeset_apply` runs inside a savepoint, so an aborted
/// apply leaves `ours` untouched.
///
/// # Errors
///
/// Every refusal maps to exit code 1 in the binary:
///
/// - [`MergeError::SchemaDiverged`]: the two sides' `sqlite_schema` differs.
/// - [`MergeError::BaseSchemaDiverged`]: the sides agree but base's schema
///   differs (the session diff needs identical table definitions).
/// - [`MergeError::MissingPrimaryKey`]: a user table has no explicit PK.
/// - [`MergeError::Conflicts`]: row-level conflicts under the abort policy.
/// - [`MergeError::IntegrityCheckFailed`] / [`MergeError::ForeignKeyCheckFailed`]:
///   the post-merge `PRAGMA` sweeps found violations.
/// - [`MergeError::Sqlite`]: any underlying `SQLite` failure.
pub fn merge(
    base_path: &str,
    ours_path: &str,
    theirs_path: &str,
    policy: ConflictPolicy,
) -> Result<()> {
    // Open theirs as the working connection so we can diff base against it.
    let theirs = Connection::open(theirs_path)?;
    let ours = Connection::open(ours_path)?;

    // Gate 1: schema must match (ignoring whitespace). Changesets are data-only.
    // The base must share the schema too, or the session diff below would fail
    // with a raw SQLITE_SCHEMA error instead of a typed refusal.
    schema::assert_schema_matches(&ours, &theirs)?;
    {
        let base = Connection::open(base_path)?;
        schema::assert_base_schema_matches(&base, &theirs)?;
    }

    // Gate 2: every user table needs an explicit PRIMARY KEY, else the session
    // extension silently skips it (silent data loss).
    schema::assert_all_tables_have_primary_key(&ours)?;
    schema::assert_all_tables_have_primary_key(&theirs)?;

    // Compute changeset base -> theirs, one table at a time. `Session::diff`
    // records the delta that turns the ATTACHed `base` schema into the
    // session's own connection (theirs).
    theirs.execute_batch(&format!(
        "ATTACH DATABASE {} AS base",
        quote_string_literal(base_path)
    ))?;
    let changeset = {
        let mut session = Session::new(&theirs)?;
        for table in schema::user_tables(&theirs)? {
            // `diff` has an unused generic `D` that cannot be inferred; pin it.
            session.diff::<&str, &str>("base", &table)?;
        }
        session.changeset()?
    };
    theirs.execute_batch("DETACH DATABASE base")?;

    // Apply the changeset to ours. The conflict handler captures every conflict
    // and, under the Abort policy, aborts the whole apply so nothing is written.
    let conflicts: Arc<Mutex<Vec<RowConflict>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&conflicts);
    let conflict_handler = move |kind: ConflictType, item: ChangesetItem| -> ConflictAction {
        if is_benign_duplicate_insert(&kind, &item) || is_convergent_delete(&kind, &item) {
            return ConflictAction::SQLITE_CHANGESET_OMIT;
        }
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(describe_conflict(&kind, &item));
        match policy {
            ConflictPolicy::Abort => ConflictAction::SQLITE_CHANGESET_ABORT,
        }
    };

    let filter: Option<fn(&str) -> bool> = None;
    let apply_result = ours.apply(&changeset, filter, conflict_handler);

    // Surface captured conflicts as a typed error. An aborted apply also
    // returns an Err from `apply`; the per-row report is the richer signal.
    let captured = conflicts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if !captured.is_empty() {
        return Err(MergeError::Conflicts(captured));
    }
    apply_result?;

    // Post-merge: fail loudly on any corruption or FK violation. No fallbacks.
    assert_integrity(&ours)?;
    assert_foreign_keys(&ours)?;

    Ok(())
}

/// `PRAGMA integrity_check` returns a single row "ok" on success, else one row
/// per problem.
fn assert_integrity(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA integrity_check")?;
    let rows: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;

    if rows == ["ok"] {
        Ok(())
    } else {
        Err(MergeError::IntegrityCheckFailed(rows))
    }
}

/// `PRAGMA foreign_key_check` returns zero rows when clean, else one row per
/// violation (table, rowid, referenced table, FK index).
fn assert_foreign_keys(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let rows: Vec<String> = stmt
        .query_map([], |row| {
            let table: String = row.get(0)?;
            let rowid: Option<i64> = row.get(1)?;
            let parent: String = row.get(2)?;
            let fkid: i64 = row.get(3)?;
            let rowid_text = rowid.map_or_else(|| "?".to_string(), |r| r.to_string());
            Ok(format!(
                "table {table} rowid {rowid_text} violates FK #{fkid} -> {parent}"
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;

    if rows.is_empty() {
        Ok(())
    } else {
        Err(MergeError::ForeignKeyCheckFailed(rows))
    }
}

/// Quote a path as a SQL string literal (single quotes doubled) for use in an
/// `ATTACH` statement. Paths from git are trusted, but quoting keeps us correct
/// for paths containing quotes.
fn quote_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
