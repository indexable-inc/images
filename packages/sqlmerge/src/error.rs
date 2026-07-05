//! Typed errors for the merge driver.
//!
//! Every variant maps to a loud, human-readable stderr message and exit code 1.
//! There is no catch-all string error: a merge driver that fails silently or
//! ambiguously is a data-loss hazard, so each refusal names exactly what went
//! wrong.

use std::fmt;

/// A row's primary key rendered for display (one column per element).
#[derive(Debug, Clone)]
pub struct PrimaryKey(pub Vec<String>);

impl fmt::Display for PrimaryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({})", self.0.join(", "))
    }
}

/// One row-level conflict surfaced by the changeset-apply conflict handler.
#[derive(Debug, Clone)]
pub struct RowConflict {
    pub table: String,
    pub kind: String,
    pub primary_key: PrimaryKey,
    /// Column-by-column "ours" value at the point of conflict, when available.
    pub ours: Vec<String>,
    /// Column-by-column incoming "theirs" value, when available.
    pub theirs: Vec<String>,
}

impl fmt::Display for RowConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ours = self.ours.join(", ");
        let theirs = self.theirs.join(", ");
        write!(
            f,
            "table {} pk {} [{}]: ours=[{ours}] theirs=[{theirs}]",
            self.table, self.primary_key, self.kind
        )
    }
}

/// A single schema object whose SQL diverges between ours and theirs.
#[derive(Debug, Clone)]
pub struct SchemaDivergence {
    pub object: String,
    pub ours: Option<String>,
    pub theirs: Option<String>,
}

/// Everything that can make a merge refuse (all map to exit code 1).
#[derive(Debug)]
pub enum MergeError {
    /// A database file could not be opened or read.
    Sqlite(rusqlite::Error),
    /// `sqlite_schema` diverges between ours and theirs; DDL is never merged.
    SchemaDiverged(Vec<SchemaDivergence>),
    /// Ours and theirs agree, but the merge base's schema differs (e.g. both
    /// sides applied the same DDL migration). `sqlite3session_diff` needs
    /// identical table definitions on both ends, so this cannot be merged.
    BaseSchemaDiverged(Vec<String>),
    /// One or more user tables lack an explicit `PRIMARY KEY`. The session
    /// extension silently skips such tables, which would be silent data loss.
    MissingPrimaryKey(Vec<String>),
    /// The changeset apply hit row conflicts; the default policy aborts.
    Conflicts(Vec<RowConflict>),
    /// `PRAGMA integrity_check` reported corruption after a clean apply.
    IntegrityCheckFailed(Vec<String>),
    /// `PRAGMA foreign_key_check` reported violations after a clean apply.
    ForeignKeyCheckFailed(Vec<String>),
}

impl From<rusqlite::Error> for MergeError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl std::error::Error for MergeError {}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Self::SchemaDiverged(objs) => {
                writeln!(
                    f,
                    "refusing to merge: schema (sqlite_schema) diverges between the two sides."
                )?;
                writeln!(
                    f,
                    "sqlmerge merges data (rows), never DDL. Resolve the schema by hand first."
                )?;
                writeln!(f, "diverging objects:")?;
                for o in objs {
                    let ours = o.ours.as_deref().unwrap_or("<absent>");
                    let theirs = o.theirs.as_deref().unwrap_or("<absent>");
                    writeln!(
                        f,
                        "  - {}\n      ours:   {ours}\n      theirs: {theirs}",
                        o.object
                    )?;
                }
                Ok(())
            }
            Self::BaseSchemaDiverged(objects) => {
                writeln!(
                    f,
                    "refusing to merge: the merge base's schema differs from the two sides \
                     (did both sides apply the same DDL migration?)."
                )?;
                writeln!(
                    f,
                    "sqlmerge merges data (rows), never DDL; the session diff needs all three \
                     versions to share a schema."
                )?;
                writeln!(f, "objects that changed since base:")?;
                for o in objects {
                    writeln!(f, "  - {o}")?;
                }
                Ok(())
            }
            Self::MissingPrimaryKey(tables) => {
                writeln!(
                    f,
                    "refusing to merge: {} table(s) lack an explicit PRIMARY KEY.",
                    tables.len()
                )?;
                writeln!(
                    f,
                    "the SQLite session extension silently skips PK-less tables, \
                     which would be silent data loss."
                )?;
                writeln!(f, "tables:")?;
                for t in tables {
                    writeln!(f, "  - {t}")?;
                }
                Ok(())
            }
            Self::Conflicts(conflicts) => {
                writeln!(
                    f,
                    "merge conflict: {} row(s) could not be merged automatically.",
                    conflicts.len()
                )?;
                for c in conflicts {
                    writeln!(f, "  - {c}")?;
                }
                Ok(())
            }
            Self::IntegrityCheckFailed(rows) => {
                writeln!(f, "post-merge PRAGMA integrity_check failed:")?;
                for r in rows {
                    writeln!(f, "  - {r}")?;
                }
                Ok(())
            }
            Self::ForeignKeyCheckFailed(rows) => {
                writeln!(f, "post-merge PRAGMA foreign_key_check found violations:")?;
                for r in rows {
                    writeln!(f, "  - {r}")?;
                }
                Ok(())
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, MergeError>;
