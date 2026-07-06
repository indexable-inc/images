//! Schema gate and primary-key gate.
//!
//! Changesets produced by the session extension are data-only: they carry row
//! inserts/updates/deletes keyed by primary key, and nothing about the table
//! definitions. Three consequences drive this module:
//!
//!   1. If the schema diverges between the merged sides, applying a data
//!      changeset across that boundary is meaningless (or corrupting), so we
//!      refuse. We compare the SQL text of every schema object,
//!      whitespace- and comment-insensitively (outside quoted literals).
//!   2. The merge base must share that schema too: `sqlite3session_diff`
//!      requires identical table definitions on both ends of the diff, so a
//!      base behind a DDL migration (even one both sides applied identically)
//!      is a typed refusal, not a raw `SQLite` error.
//!   3. The session extension silently skips any table without an explicit
//!      `PRIMARY KEY`. Merging such a table would drop changes with no error,
//!      so we refuse loudly instead.

use std::collections::BTreeMap;

use rusqlite::Connection;

use crate::error::{MergeError, Result, SchemaDivergence};

/// Normalize schema SQL for whitespace-insensitive comparison: outside quoted
/// regions, collapse runs of whitespace to a single space and drop spaces
/// adjacent to structural punctuation (`(`, `)`, `,`). This makes
/// `CREATE TABLE t (id ...)` and `CREATE TABLE t( id ... )` compare equal,
/// since pure reformatting is not a schema change, while any token difference
/// (an added column, a changed type) still shows up.
///
/// Quoted regions are copied verbatim: `'...'` string literals (a changed
/// `DEFAULT 'a, b'` is a real schema change), `"..."`/`` `...` `` quoted
/// identifiers, and `[...]` bracket identifiers. Doubled closing quotes
/// (`''`, `""`) are the SQL escape and stay inside the region.
///
/// SQL comments are dropped entirely, since a comment is never a schema change
/// (`SQLite` stores them verbatim in `sqlite_schema.sql`): line comments (`--` to
/// end of line) and block comments (`/* ... */`, which do not nest). A comment
/// is only recognized outside a quoted region, so a `--` or `/*` inside a
/// literal stays literal. A dropped comment collapses like whitespace, so it
/// separates adjacent tokens (`a/* c */b` becomes `a b`) rather than gluing
/// them.
fn normalize_sql(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    let mut closing_quote: Option<char> = None;

    while let Some(ch) = chars.next() {
        if let Some(q) = closing_quote {
            out.push(ch);
            if ch == q {
                if chars.peek() == Some(&q) {
                    // Doubled quote: the escape for a literal quote character.
                    // Copy it and stay inside the quoted region.
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                } else {
                    closing_quote = None;
                }
            }
            continue;
        }

        // Comments (only outside quoted regions) are dropped and treated as a
        // whitespace boundary: emit at most one separating space, then let the
        // normal whitespace-collapse / punctuation rules decide if it survives.
        if ch == '-' && chars.peek() == Some(&'-') {
            chars.next(); // consume the second '-'
            for c in chars.by_ref() {
                if c == '\n' {
                    break;
                }
            }
            if !out.is_empty() && !out.ends_with([' ', '(', ',']) {
                out.push(' ');
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume the '*'
            // Scan to the closing `*/` (block comments do not nest in SQLite).
            let mut prev = '\0';
            for c in chars.by_ref() {
                if prev == '*' && c == '/' {
                    break;
                }
                prev = c;
            }
            if !out.is_empty() && !out.ends_with([' ', '(', ',']) {
                out.push(' ');
            }
            continue;
        }

        match ch {
            '\'' | '"' | '`' => {
                closing_quote = Some(ch);
                out.push(ch);
            }
            '[' => {
                closing_quote = Some(']');
                out.push(ch);
            }
            c if c.is_whitespace() => {
                // Collapse to one space; suppress it entirely at the start or
                // after an opening paren / comma.
                if !out.is_empty() && !out.ends_with([' ', '(', ',']) {
                    out.push(' ');
                }
            }
            '(' | ')' | ',' => {
                // Drop the space before structural punctuation.
                if out.ends_with(' ') {
                    out.pop();
                }
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }

    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Map of schema object name to its (normalized) defining SQL.
///
/// We read from `sqlite_schema` (aka `sqlite_master`). Rows with NULL `sql`
/// (auto-created indexes for `PRIMARY KEY` / `UNIQUE`) are excluded: they are
/// derived from the table definitions we already compare, and `SQLite` does not
/// store SQL for them.
fn schema_objects(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut stmt = conn.prepare(
        "SELECT name, sql FROM sqlite_schema \
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )?;
    let map = stmt
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let sql: String = row.get(1)?;
            Ok((name, normalize_sql(&sql)))
        })?
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    Ok(map)
}

/// Every schema object whose (normalized) SQL differs between `a` and `b`,
/// including objects present on only one side.
fn diverging_objects(a: &Connection, b: &Connection) -> Result<Vec<SchemaDivergence>> {
    let a = schema_objects(a)?;
    let b = schema_objects(b)?;

    let mut names: Vec<&String> = a.keys().chain(b.keys()).collect();
    names.sort_unstable();
    names.dedup();

    let mut divergences = Vec::new();
    for name in names {
        let a_sql = a.get(name);
        let b_sql = b.get(name);
        if a_sql != b_sql {
            divergences.push(SchemaDivergence {
                object: name.clone(),
                ours: a_sql.cloned(),
                theirs: b_sql.cloned(),
            });
        }
    }
    Ok(divergences)
}

/// Refuse if the schema of `ours` and `theirs` diverges (whitespace-insensitive).
///
/// # Errors
///
/// Returns [`MergeError::SchemaDiverged`] listing every object whose SQL
/// differs (or exists on only one side), or [`MergeError::Sqlite`] if either
/// schema cannot be read.
pub fn assert_schema_matches(ours: &Connection, theirs: &Connection) -> Result<()> {
    let divergences = diverging_objects(ours, theirs)?;
    if divergences.is_empty() {
        Ok(())
    } else {
        Err(MergeError::SchemaDiverged(divergences))
    }
}

/// Refuse if the merge base's schema differs from the (matching) sides.
///
/// `sqlite3session_diff` requires the same table definition on both ends, so
/// without this gate a schema-migrated base surfaces as a raw `SQLITE_SCHEMA`
/// error mid-diff instead of a typed refusal.
///
/// # Errors
///
/// Returns [`MergeError::BaseSchemaDiverged`] naming the objects whose schema
/// changed relative to base, or [`MergeError::Sqlite`] if either schema
/// cannot be read.
pub fn assert_base_schema_matches(base: &Connection, side: &Connection) -> Result<()> {
    let divergences = diverging_objects(base, side)?;
    if divergences.is_empty() {
        Ok(())
    } else {
        let objects = divergences.into_iter().map(|d| d.object).collect();
        Err(MergeError::BaseSchemaDiverged(objects))
    }
}

/// Names of user tables (excludes `sqlite_%` internal tables and views).
///
/// # Errors
///
/// Returns [`MergeError::Sqlite`] if the schema cannot be read.
pub fn user_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_schema \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )?;
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(tables)
}

/// Return true if `table` has at least one column flagged as part of the
/// primary key. `PRAGMA table_info` sets `pk` > 0 for PK columns.
///
/// A `rowid` alias declared `INTEGER PRIMARY KEY` reports `pk = 1`, which is
/// exactly the case the session extension handles, so it correctly counts as
/// having a PK. A `WITHOUT ROWID` table always has a PK by definition. Only a
/// plain table with no `PRIMARY KEY` clause (rows keyed by the implicit rowid,
/// which the session extension cannot address) reports no `pk` column.
fn table_has_primary_key(conn: &Connection, table: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE pk > 0")?;
    let count: i64 = stmt.query_row([table], |row| row.get(0))?;
    Ok(count > 0)
}

/// Refuse if any user table lacks an explicit `PRIMARY KEY`.
///
/// # Errors
///
/// Returns [`MergeError::MissingPrimaryKey`] naming every PK-less table, or
/// [`MergeError::Sqlite`] if the schema cannot be read.
pub fn assert_all_tables_have_primary_key(conn: &Connection) -> Result<()> {
    let mut missing = Vec::new();
    for table in user_tables(conn)? {
        if !table_has_primary_key(conn, &table)? {
            missing.push(table);
        }
    }

    if missing.is_empty() {
        Ok(())
    } else {
        Err(MergeError::MissingPrimaryKey(missing))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_sql;

    #[test]
    fn collapses_whitespace_outside_quotes() {
        assert_eq!(
            normalize_sql("CREATE TABLE t (\n  id  INTEGER  PRIMARY KEY ,\n  v TEXT\n)"),
            normalize_sql("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)"),
        );
    }

    #[test]
    fn preserves_whitespace_inside_string_literals() {
        let spaced = normalize_sql("CREATE TABLE t (v TEXT DEFAULT 'a, b')");
        let tight = normalize_sql("CREATE TABLE t (v TEXT DEFAULT 'a,b')");
        assert_ne!(spaced, tight);
        assert!(spaced.contains("'a, b'"));
    }

    #[test]
    fn doubled_quote_escape_stays_inside_literal() {
        // 'it''s  x' contains an escaped quote; the run of spaces after it is
        // still inside the literal and must survive.
        let sql = "CREATE TABLE t (v TEXT DEFAULT 'it''s  x')";
        assert!(normalize_sql(sql).contains("'it''s  x'"));
    }

    #[test]
    fn preserves_quoted_identifiers() {
        let quoted = normalize_sql("CREATE TABLE \"my  table\" (id INTEGER PRIMARY KEY)");
        assert!(quoted.contains("\"my  table\""));
        let bracket = normalize_sql("CREATE TABLE [my  table] (id INTEGER PRIMARY KEY)");
        assert!(bracket.contains("[my  table]"));
    }

    #[test]
    fn line_comment_does_not_diverge() {
        let none = normalize_sql("CREATE TABLE t (id INTEGER PRIMARY KEY)");
        let v1 = normalize_sql("CREATE TABLE t (id INTEGER PRIMARY KEY) -- v1");
        let v2 = normalize_sql("CREATE TABLE t (id INTEGER PRIMARY KEY) -- v2");
        assert_eq!(v1, v2);
        assert_eq!(v1, none);
    }

    #[test]
    fn block_comment_does_not_diverge() {
        let none = normalize_sql("CREATE TABLE t (id INTEGER PRIMARY KEY)");
        // Mid-statement block comment must vanish without changing the schema.
        let embedded = normalize_sql("CREATE TABLE t (id INTEGER /* pk */ PRIMARY KEY)");
        assert_eq!(embedded, none);
    }

    #[test]
    fn comment_does_not_glue_adjacent_tokens() {
        let out = normalize_sql("CREATE TABLE t (a/* x */b TEXT)");
        assert!(out.contains("a b"), "tokens glued: {out}");
        assert!(!out.contains("ab"), "tokens glued: {out}");
    }

    #[test]
    fn double_dash_inside_string_literal_is_not_a_comment() {
        let out = normalize_sql("CREATE TABLE t (v TEXT DEFAULT '-- not a comment')");
        assert!(out.contains("'-- not a comment'"), "literal mangled: {out}");
    }

    #[test]
    fn punctuation_inside_comment_does_not_affect_real_punctuation() {
        // A `,` inside a comment must be dropped with the comment and must not
        // interfere with structural-punctuation handling of the real commas.
        let with_comment = normalize_sql("CREATE TABLE t (a TEXT, /* one, two, three */ b TEXT)");
        let without = normalize_sql("CREATE TABLE t (a TEXT, b TEXT)");
        assert_eq!(with_comment, without);
    }
}
