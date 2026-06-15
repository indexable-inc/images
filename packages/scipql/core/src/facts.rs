//! Lower a SCIP index into Soufflé facts.
//!
//! Each occurrence becomes one `occurrence(symbol, path, start, end, role)` row
//! keyed by the SCIP *symbol* (the fully-qualified moniker), so two same-named
//! definitions in different modules are distinct rows. `start`/`end` are
//! absolute byte offsets into the file (computed from the SCIP line/column and
//! the document text, which rust-analyzer emits UTF-8 encoded), so the `edit`
//! relation a query derives feeds `edit-applier` directly.

use std::path::Path;

use protobuf::Message as _;
use scip::types::{Index, SymbolInformation};
use snafu::ResultExt as _;

use crate::error::{
    Error, OffsetSnafu, ParseIndexSnafu, ReadIndexSnafu, ReadSourceSnafu, WriteFactsSnafu,
};

/// The schema (`.decl` + `.input`) for every relation [`Facts`] emits. Prepended
/// to a user's Soufflé program so their rules can read these relations without
/// redeclaring them.
pub const SCHEMA: &str = "\
.decl occurrence(symbol:symbol, path:symbol, start:number, end:number, role:symbol)
.input occurrence
.decl symbol_info(symbol:symbol, kind:symbol, display_name:symbol)
.input symbol_info
.decl document(path:symbol)
.input document
.decl relationship(symbol:symbol, related:symbol, kind:symbol)
.input relationship
";

/// `occurrence(symbol, path, start, end, role)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceRow {
    pub symbol: String,
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub role: String,
}

/// `symbol_info(symbol, kind, display_name)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRow {
    pub symbol: String,
    pub kind: String,
    pub display_name: String,
}

/// `relationship(symbol, related, kind)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationshipRow {
    pub symbol: String,
    pub related: String,
    pub kind: String,
}

/// Every relation lowered from one SCIP index.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Facts {
    pub occurrences: Vec<OccurrenceRow>,
    pub symbols: Vec<SymbolRow>,
    pub documents: Vec<String>,
    pub relationships: Vec<RelationshipRow>,
}

/// Read and decode a SCIP protobuf index from disk.
///
/// # Errors
///
/// Fails if the file cannot be read or is not a valid SCIP index.
pub fn load_index(path: &Path) -> Result<Index, Error> {
    let bytes = std::fs::read(path).context(ReadIndexSnafu { path })?;
    Index::parse_from_bytes(&bytes).context(ParseIndexSnafu)
}

/// Byte offset of `(line, column)` given the line-start byte offsets of a file.
///
/// SCIP positions are zero-based line and (under rust-analyzer's UTF-8 encoding)
/// byte column. Negatives are clamped to zero and a position past the end
/// clamps to the file length.
///
/// # Errors
///
/// Propagates the (practically impossible) overflow of a non-negative `i32`
/// into `usize`.
fn byte_offset(
    line_starts: &[usize],
    len: usize,
    line: i32,
    column: i32,
) -> Result<usize, std::num::TryFromIntError> {
    let line = usize::try_from(line.max(0))?;
    let column = usize::try_from(column.max(0))?;
    let line_start = line_starts.get(line).copied().unwrap_or(len);
    // Clamp to the next line's start (and the file end) so an out-of-range
    // column from a stale index can't splice across a line boundary.
    let line_end = line_starts.get(line + 1).copied().unwrap_or(len);
    Ok((line_start + column).min(line_end).min(len))
}

/// Byte offset of the start of each line (index 0 is offset 0).
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (offset, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(offset + 1);
        }
    }
    starts
}

/// SCIP packs a single-line range as `[line, startCol, endCol]` and a
/// multi-line one as `[startLine, startCol, endLine, endCol]`. Returns
/// `(start_line, start_col, end_line, end_col)`.
fn span(range: &[i32]) -> Option<Span> {
    match *range {
        [line, start_col, end_col] => Some(Span {
            start_line: line,
            start_col,
            end_line: line,
            end_col,
        }),
        [start_line, start_col, end_line, end_col] => Some(Span {
            start_line,
            start_col,
            end_line,
            end_col,
        }),
        _ => None,
    }
}

struct Span {
    start_line: i32,
    start_col: i32,
    end_line: i32,
    end_col: i32,
}

/// SCIP `SymbolRole::Definition`; the low bit of `symbol_roles`.
const DEFINITION_ROLE: i32 = 1;

/// A local symbol (`local 1`) is unique only within its document, so prefix it
/// with the path to keep facts globally unique; a global moniker is already
/// unique and passes through unchanged.
fn canonical_symbol(symbol: &str, path: &str) -> String {
    if scip::symbol::is_local_symbol(symbol) {
        format!("{path}\u{1}{symbol}")
    } else {
        symbol.to_owned()
    }
}

fn symbol_row(info: &SymbolInformation, path: &str) -> SymbolRow {
    SymbolRow {
        symbol: canonical_symbol(&info.symbol, path),
        kind: format!("{:?}", info.kind.enum_value_or_default()),
        display_name: info.display_name.clone(),
    }
}

fn relationship_rows(info: &SymbolInformation, path: &str) -> Vec<RelationshipRow> {
    let symbol = canonical_symbol(&info.symbol, path);
    let mut rows = Vec::new();
    for rel in &info.relationships {
        let related = canonical_symbol(&rel.symbol, path);
        let mut kinds = Vec::new();
        if rel.is_implementation {
            kinds.push("implementation");
        }
        if rel.is_type_definition {
            kinds.push("type_definition");
        }
        if rel.is_reference {
            kinds.push("reference");
        }
        if rel.is_definition {
            kinds.push("definition");
        }
        for kind in kinds {
            rows.push(RelationshipRow {
                symbol: symbol.clone(),
                related: related.clone(),
                kind: kind.to_owned(),
            });
        }
    }
    rows
}

/// Lower a SCIP index into [`Facts`].
///
/// Byte offsets come from each document's embedded `text`; if a document omits
/// it, the file is read from `root` (defaulting to the index's `project_root`)
/// joined with the document's relative path.
///
/// # Errors
///
/// Fails only when a document has no embedded text and its source file cannot
/// be read.
pub fn facts_from_index(index: &Index, root: Option<&Path>) -> Result<Facts, Error> {
    let root = root.map_or_else(|| project_root(index), Path::to_path_buf);
    let mut facts = Facts::default();

    for symbol in &index.external_symbols {
        facts.symbols.push(symbol_row(symbol, ""));
    }

    for document in &index.documents {
        let path = document.relative_path.clone();
        facts.documents.push(path.clone());

        let text = if document.text.is_empty() {
            let source = root.join(&path);
            std::fs::read_to_string(&source).context(ReadSourceSnafu { path: source })?
        } else {
            document.text.clone()
        };
        let starts = line_starts(&text);
        let len = text.len();

        for symbol in &document.symbols {
            facts.symbols.push(symbol_row(symbol, &path));
            facts.relationships.extend(relationship_rows(symbol, &path));
        }

        for occurrence in &document.occurrences {
            let Some(span) = span(&occurrence.range) else {
                continue;
            };
            let role = if occurrence.symbol_roles & DEFINITION_ROLE == 0 {
                "reference"
            } else {
                "definition"
            };
            facts.occurrences.push(OccurrenceRow {
                symbol: canonical_symbol(&occurrence.symbol, &path),
                path: path.clone(),
                start: byte_offset(&starts, len, span.start_line, span.start_col)
                    .context(OffsetSnafu)?,
                end: byte_offset(&starts, len, span.end_line, span.end_col).context(OffsetSnafu)?,
                role: role.to_owned(),
            });
        }
    }

    Ok(facts)
}

/// `metadata.project_root` as a filesystem path (`file://` stripped).
pub fn project_root(index: &Index) -> std::path::PathBuf {
    let root = &index.metadata.project_root;
    root.strip_prefix("file://").unwrap_or(root).into()
}

fn write_tsv(path: &Path, rows: impl Iterator<Item = Vec<String>>) -> Result<(), Error> {
    let mut out = String::new();
    for row in rows {
        // Soufflé reads facts as tab-separated lines and does not unescape, so a
        // tab/newline in a cell (reachable via an arbitrary `display_name`) would
        // shift columns or abort the load. Monikers and paths never contain
        // them; collapse any to a space so a stray one can't corrupt the table.
        let cells: Vec<String> = row
            .iter()
            .map(|cell| cell.replace(['\t', '\n', '\r'], " "))
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    std::fs::write(path, out).context(WriteFactsSnafu { path })
}

impl Facts {
    /// Write one `<relation>.facts` TSV per relation into `dir` (which must
    /// exist). Pair with [`SCHEMA`] when invoking Soufflé.
    ///
    /// # Errors
    ///
    /// Fails if any facts file cannot be written.
    pub fn write_dir(&self, dir: &Path) -> Result<(), Error> {
        write_tsv(
            &dir.join("occurrence.facts"),
            self.occurrences.iter().map(|row| {
                vec![
                    row.symbol.clone(),
                    row.path.clone(),
                    row.start.to_string(),
                    row.end.to_string(),
                    row.role.clone(),
                ]
            }),
        )?;
        write_tsv(
            &dir.join("symbol_info.facts"),
            self.symbols.iter().map(|row| {
                vec![row.symbol.clone(), row.kind.clone(), row.display_name.clone()]
            }),
        )?;
        write_tsv(
            &dir.join("document.facts"),
            self.documents.iter().map(|path| vec![path.clone()]),
        )?;
        write_tsv(
            &dir.join("relationship.facts"),
            self.relationships.iter().map(|row| {
                vec![row.symbol.clone(), row.related.clone(), row.kind.clone()]
            }),
        )
    }
}
