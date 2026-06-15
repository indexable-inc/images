//! Apply the `edit` relation a Soufflé program derives.
//!
//! A `fix` program `.output`s `edit(path, start, end, replacement)` where
//! `start`/`end` are byte offsets (the offsets [`crate::facts`] emitted). Those
//! rows become [`edit_applier::Edit`]s, checked for overlap and applied; the
//! default is a dry-run unified diff, `write` persists the files under `root`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use edit_applier::Edit;
use snafu::{OptionExt as _, ResultExt as _, ensure};

use crate::error::{BadEditRowSnafu, EditUnknownPathSnafu, Error, ReadSourceSnafu, WriteRewriteSnafu};
use crate::facts::Facts;
use crate::souffle;

/// The relation a `fix`/`rename` program must output, with byte-offset columns.
pub const EDIT_SCHEMA: &str = "\
.decl edit(path:symbol, start:number, end:number, replacement:symbol)
.output edit
";

fn parse_offset(value: &str, row: usize, column: &str) -> Result<usize, Error> {
    value.parse().ok().context(BadEditRowSnafu {
        row,
        column: column.to_owned(),
        expected: "byte offset".to_owned(),
        value: value.to_owned(),
    })
}

/// Run `program` against `facts` and apply its `edit` relation.
///
/// `program` should declare and `.output` the `edit` relation ([`EDIT_SCHEMA`]
/// is prepended for convenience). Returns the unified diff; with `write`, the
/// files (resolved as `root` joined with each edit's relative `path`) are
/// rewritten on disk.
///
/// # Errors
///
/// Fails on a Soufflé error, a malformed `edit` row, overlapping edits, or an
/// unreadable/unwritable source file.
pub fn fix(
    facts: &Facts,
    program: &str,
    root: &Path,
    write: bool,
    scratch: &Path,
) -> Result<String, Error> {
    let full = format!("{EDIT_SCHEMA}\n{program}");
    let output = souffle::run(facts, &full, scratch)?;
    let rows = output.relation("edit").map_or(&[][..], |rel| &rel.rows);

    // Confine edits to files the index actually documents. An `edit` row's
    // path is whatever the (user-supplied) program emits, and `root.join(path)`
    // would follow an absolute or `../` path outside the project; requiring
    // membership in the indexed documents (which are project-relative) blocks
    // that arbitrary-write vector. Legitimate edits target occurrence paths,
    // which are document paths, so they always pass.
    let documents: HashSet<&str> = facts.documents.iter().map(String::as_str).collect();

    let mut paths: Vec<String> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut edits = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        let [path, start, end, replacement] = row.as_slice() else {
            return BadEditRowSnafu {
                row: row_index,
                column: "arity".to_owned(),
                expected: "4 columns (path, start, end, replacement)".to_owned(),
                value: format!("{row:?}"),
            }
            .fail();
        };
        ensure!(
            documents.contains(path.as_str()),
            EditUnknownPathSnafu { path: path.clone() }
        );
        let file = *index_of.entry(path.clone()).or_insert_with(|| {
            paths.push(path.clone());
            paths.len() - 1
        });
        edits.push(Edit {
            file,
            start: parse_offset(start, row_index, "start")?,
            end: parse_offset(end, row_index, "end")?,
            replacement: replacement.clone(),
        });
    }

    let files = paths
        .iter()
        .map(|path| {
            let absolute = root.join(path);
            let text = std::fs::read_to_string(&absolute)
                .context(ReadSourceSnafu { path: absolute })?;
            Ok((PathBuf::from(path), text))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let path_bufs: Vec<PathBuf> = files.iter().map(|(path, _)| path.clone()).collect();

    edits.sort();
    edits.dedup();
    edit_applier::check_overlaps(&path_bufs, &edits)?;
    let rewrites = edit_applier::apply(&files, &edits);
    let diff = edit_applier::unified_diff(&files, &rewrites);

    if write {
        for rewrite in &rewrites {
            let absolute = root.join(&rewrite.path);
            std::fs::write(&absolute, &rewrite.content)
                .context(WriteRewriteSnafu { path: absolute })?;
        }
    }
    Ok(diff)
}
