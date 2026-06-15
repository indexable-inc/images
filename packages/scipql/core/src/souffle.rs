//! Run Soufflé over lowered [`Facts`] and read its output relations back.
//!
//! The schema (`.decl`/`.input` for every fact relation, see [`crate::SCHEMA`])
//! is prepended to the caller's program, so a query only writes the rules and
//! `.output` directives it cares about. Output relations come back as plain
//! string rows; column names are recovered from the program's `.decl` lines.

use std::path::Path;
use std::process::Command;

use snafu::{ResultExt as _, ensure};

use crate::error::{Error, ReadOutputSnafu, RunSouffleSnafu, SouffleFailedSnafu};
use crate::facts::{Facts, SCHEMA};

/// One Soufflé output relation: its name, column names (from the `.decl`), and
/// rows (each cell a string, in column order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRelation {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Every relation Soufflé wrote (one per `.output`), sorted by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryOutput {
    pub relations: Vec<OutputRelation>,
}

impl QueryOutput {
    /// The named relation, if the program `.output`-ed it.
    #[must_use]
    pub fn relation(&self, name: &str) -> Option<&OutputRelation> {
        self.relations.iter().find(|relation| relation.name == name)
    }
}

/// Run `program` (the user's rules) against `facts` and return every `.output`
/// relation. `dir` must be an existing empty directory used as scratch.
///
/// # Errors
///
/// Fails if facts cannot be written, Soufflé cannot be spawned or exits
/// non-zero, or an output CSV cannot be read.
pub fn run(facts: &Facts, program: &str, dir: &Path) -> Result<QueryOutput, Error> {
    let facts_dir = dir.join("facts");
    let out_dir = dir.join("out");
    std::fs::create_dir_all(&facts_dir).context(crate::error::WriteFactsSnafu { path: &facts_dir })?;
    std::fs::create_dir_all(&out_dir).context(crate::error::WriteFactsSnafu { path: &out_dir })?;
    facts.write_dir(&facts_dir)?;

    let full_program = format!("{SCHEMA}\n{program}\n");
    let program_path = dir.join("query.dl");
    std::fs::write(&program_path, &full_program)
        .context(crate::error::WriteFactsSnafu { path: &program_path })?;

    // `SCIPQL_SOUFFLE` lets a wrapper bake an absolute path (the CLI and the
    // mcp interpreter do this) so the tool resolves without relying on PATH
    // order; falls back to `souffle` on PATH.
    let souffle = std::env::var("SCIPQL_SOUFFLE").unwrap_or_else(|_| "souffle".to_owned());
    let output = Command::new(souffle)
        .arg("-F")
        .arg(&facts_dir)
        .arg("-D")
        .arg(&out_dir)
        .arg(&program_path)
        .output()
        .context(RunSouffleSnafu)?;
    ensure!(
        output.status.success(),
        SouffleFailedSnafu {
            code: output
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    );

    let columns = declared_columns(&full_program);
    let mut relations = Vec::new();
    let entries = std::fs::read_dir(&out_dir).context(ReadOutputSnafu { path: &out_dir })?;
    for entry in entries {
        let entry = entry.context(ReadOutputSnafu { path: &out_dir })?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "csv") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let body = std::fs::read_to_string(&path).context(ReadOutputSnafu { path: &path })?;
        let rows = body
            .lines()
            .map(|line| line.split('\t').map(str::to_owned).collect())
            .collect();
        relations.push(OutputRelation {
            name: name.to_owned(),
            columns: columns.get(name).cloned().unwrap_or_default(),
            rows,
        });
    }
    relations.sort_by(|first, second| first.name.cmp(&second.name));
    Ok(QueryOutput { relations })
}

/// Map every `.decl <name>(col:type, ...)` in `program` to its column names.
fn declared_columns(program: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    for line in program.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(".decl ") else {
            continue;
        };
        let Some(open) = rest.find('(') else {
            continue;
        };
        let Some(close) = rest.find(')') else {
            continue;
        };
        let name = rest[..open].trim().to_owned();
        let columns = rest[open + 1..close]
            .split(',')
            .filter_map(|field| field.split(':').next())
            .map(|column| column.trim().to_owned())
            .filter(|column| !column.is_empty())
            .collect();
        map.insert(name, columns);
    }
    map
}
