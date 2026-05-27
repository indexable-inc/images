use crate::{
    error::{self, Error, Result},
    types::{IndexSchema, IndexStats},
};
use repo_walker::{FileScanner, WalkOptions};
use snafu::ResultExt;
use std::path::Path;
use tantivy::{IndexWriter, Term, doc};

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

pub fn chunk_content(content: &str) -> Vec<(usize, String)> {
    if content.len() <= CHUNK_SIZE {
        return vec![(0, content.to_string())];
    }

    let mut chunks = Vec::new();
    let mut offset = 0;

    while offset < content.len() {
        let end = (offset + CHUNK_SIZE).min(content.len());
        let end = content
            .char_indices()
            .find(|(i, _)| *i >= end)
            .map_or(content.len(), |(i, _)| i);
        let chunk = content.get(offset..end).unwrap_or("");
        chunks.push((offset, chunk.to_string()));

        if end >= content.len() {
            break;
        }

        let next_offset = offset + CHUNK_SIZE - CHUNK_OVERLAP;
        offset = content
            .char_indices()
            .find(|(i, _)| *i >= next_offset)
            .map_or(content.len(), |(i, _)| i);
    }

    chunks
}

pub fn index_directory(
    writer: &mut IndexWriter,
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
        match entry {
            Ok(file_path) => match index_file(writer, schema, &file_path) {
                Ok(()) => stats.files_indexed += 1,
                Err(e) => {
                    stats.files_skipped += 1;
                    stats.errors.push((file_path, e.to_string()));
                }
            },
            Err(err) => {
                stats.files_skipped += 1;
                stats.errors.push((directory.to_path_buf(), format!("walker: {err}")));
            }
        }
    }

    writer.commit().context(error::CommitIndexSnafu)?;
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

    for (offset, chunk) in chunk_content(&content) {
        writer
            .add_document(doc!(
                schema.path => path_str.clone(),
                schema.path_exact => path_str.clone(),
                schema.content => chunk,
                schema.filename => filename,
                schema.chunk_offset => offset as u64,
                schema.directory => directory_value.clone(),
                schema.extension => extension.clone(),
            ))
            .context(error::CreateIndexSnafu)?;
    }

    Ok(())
}
