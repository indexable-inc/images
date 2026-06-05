use crate::{
    error::{self, Error, Result},
    types::{IndexSchema, IndexStats},
};
use repo_walker::{FileScanner, WalkOptions};
use snafu::ResultExt;
use std::ops::Bound;
use std::path::Path;
use tantivy::{IndexWriter, Term, doc, query::RangeQuery};

const MAX_FILE_SIZE: u64 = 1_048_576;
const CHUNK_SIZE: usize = 500;
const CHUNK_OVERLAP: usize = 100;

/// Encode a directory path for the keyword `directory` field with a trailing
/// path separator. The trailing separator lets the search side use a tight
/// byte range `[dir + '/', dir + '0')` to match `dir` itself plus every
/// descendant, without catching same-prefix siblings like `dir-old`.
pub fn directory_term(path: &Path) -> String {
    let mut s = path.to_string_lossy().into_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// One chunk of a file's content: its starting byte offset and the chunk text.
pub struct ContentChunk {
    pub offset: usize,
    pub text: String,
}

pub fn chunk_content(content: &str) -> Vec<ContentChunk> {
    if content.len() <= CHUNK_SIZE {
        return vec![ContentChunk {
            offset: 0,
            text: content.to_string(),
        }];
    }

    // Materialize char boundaries once so each chunk's end + next-start lookup
    // is O(log n) via binary search instead of restarting `char_indices` from
    // byte 0 every iteration (which made the loop quadratic on large files).
    let boundaries: Vec<usize> = content
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(content.len()))
        .collect();
    let aligned = |raw: usize| -> usize {
        if raw >= content.len() {
            return content.len();
        }
        let idx = boundaries
            .binary_search(&raw)
            .unwrap_or_else(|insert| insert);
        boundaries.get(idx).copied().unwrap_or(content.len())
    };

    let mut chunks = Vec::new();
    let mut offset = 0;

    while offset < content.len() {
        let end = aligned(offset + CHUNK_SIZE);
        let chunk = content.get(offset..end).unwrap_or("");
        chunks.push(ContentChunk {
            offset,
            text: chunk.to_string(),
        });

        if end >= content.len() {
            break;
        }
        offset = aligned(offset + CHUNK_SIZE - CHUNK_OVERLAP);
    }

    chunks
}

pub fn index_directory(
    writer: &mut IndexWriter,
    schema: &IndexSchema,
    directory: &Path,
    respect_gitignore: bool,
) -> Result<IndexStats> {
    // Wipe every doc whose `directory` lives under the indexed root so
    // files that were deleted, renamed, or newly gitignored between runs
    // disappear from search results. The trailing-slash encoding plus the
    // `[<root>/, <root>0)` range is the same trick `search::search` uses,
    // and only matches files actually under this root — sibling roots like
    // `<root>-old` stay untouched.
    let canonical_root = std::fs::canonicalize(directory)
        .context(error::CanonicalizeSnafu { path: directory })?;
    let root_lower = directory_term(&canonical_root);
    let mut root_upper = root_lower.clone();
    root_upper.pop();
    root_upper.push('0');
    let lower_term = Term::from_field_text(schema.directory, &root_lower);
    let upper_term = Term::from_field_text(schema.directory, &root_upper);
    let cleanup = RangeQuery::new(Bound::Included(lower_term), Bound::Excluded(upper_term));
    writer
        .delete_query(Box::new(cleanup))
        .context(error::CommitIndexSnafu)?;

    match walk_and_add(writer, schema, directory, respect_gitignore) {
        Ok(stats) => {
            writer.commit().context(error::CommitIndexSnafu)?;
            Ok(stats)
        }
        Err(err) => {
            // The queued wipe + any added docs are still uncommitted in the
            // writer. Roll back so a transient walker failure doesn't blow
            // away the previous index on the next commit. `rollback` only
            // surfaces a fault when the writer state itself is broken, so
            // we report the original walk error either way.
            let _ = writer.rollback();
            Err(err)
        }
    }
}

fn walk_and_add(
    writer: &IndexWriter,
    schema: &IndexSchema,
    directory: &Path,
    respect_gitignore: bool,
) -> Result<IndexStats> {
    let scanner = FileScanner::new(
        directory,
        WalkOptions {
            respect_gitignore,
            follow_links: false,
        },
    );
    let mut stats = IndexStats::default();

    for entry in scanner {
        // Walker errors (missing root, permission-denied subtree) abort the
        // run before commit so the wipe above is rolled back. Per-file
        // read/parse errors stay non-fatal and are recorded in stats.
        let file_path = entry.context(error::WalkSnafu { directory })?;
        match index_file(writer, schema, &file_path) {
            Ok(()) => stats.files_indexed += 1,
            Err(e) => {
                stats.files_skipped += 1;
                stats.errors.push((file_path, e.to_string()));
            }
        }
    }
    Ok(stats)
}

fn index_file(writer: &IndexWriter, schema: &IndexSchema, file_path: &Path) -> Result<()> {
    let metadata =
        std::fs::metadata(file_path).context(error::GetMetadataSnafu { path: file_path })?;

    if metadata.len() > MAX_FILE_SIZE {
        return Err(Error::FileTooLarge {
            path: file_path.to_path_buf(),
            size: metadata.len(),
            max_size: MAX_FILE_SIZE,
        });
    }

    let content =
        std::fs::read_to_string(file_path).context(error::ReadFileSnafu { path: file_path })?;

    // Canonicalize the file path once and use it for every field so a
    // later re-index with a differently-spelled equivalent path (relative
    // vs absolute, with or without `.`, through a symlinked ancestor) lines
    // up with the previous run's `path_exact` term and the parent-directory
    // facet. Without this, the delete misses and stale chunks pile up.
    let canonical_file = std::fs::canonicalize(file_path)
        .context(error::CanonicalizeSnafu { path: file_path })?;
    let canonical_parent =
        canonical_file
            .parent()
            .ok_or_else(|| Error::IndexedPathHasNoParent {
                path: canonical_file.clone(),
            })?;

    let filename = canonical_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let path_str = canonical_file.to_string_lossy().into_owned();
    let directory_value = directory_term(canonical_parent);
    let extension = canonical_file
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_owned();

    // `path_exact` is the untokenized keyword copy; this delete actually
    // matches the previous chunks recorded for `file_path`. Deleting via
    // `schema.path` would silently no-op because that field is stemmed.
    writer.delete_term(Term::from_field_text(schema.path_exact, &path_str));

    for ContentChunk { offset, text } in chunk_content(&content) {
        writer
            .add_document(doc!(
                schema.path => path_str.clone(),
                schema.path_exact => path_str.clone(),
                schema.content => text,
                schema.filename => filename,
                schema.chunk_offset => offset as u64,
                schema.directory => directory_value.clone(),
                schema.extension => extension.clone(),
            ))
            .context(error::CreateIndexSnafu)?;
    }

    Ok(())
}
