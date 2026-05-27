use crate::{
    error::{self, Error, Result},
    types::{IndexSchema, IndexStats},
};
use repo_walker::{FileScanner, WalkOptions};
use snafu::ResultExt;
use std::path::Path;
use tantivy::{IndexWriter, Term, doc, schema::Facet};

const MAX_FILE_SIZE: u64 = 1_048_576;
const CHUNK_SIZE: usize = 500;
const CHUNK_OVERLAP: usize = 100;

fn path_to_facet(path: &Path) -> Facet {
    let path_str = path.to_string_lossy();
    let normalized = if path_str.starts_with('/') {
        path_str.into_owned()
    } else {
        format!("/{path_str}")
    };
    Facet::from(&normalized)
}

fn extension_to_facet(extension: &str) -> Facet {
    if extension.is_empty() {
        Facet::from("/")
    } else {
        Facet::from(&format!("/{extension}"))
    }
}

pub(crate) fn chunk_content(content: &str) -> Vec<(usize, String)> {
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

pub(crate) fn index_directory(
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

    for file_path in scanner {
        match index_file(writer, schema, &file_path) {
            Ok(()) => stats.files_indexed += 1,
            Err(e) => {
                stats.files_skipped += 1;
                stats.errors.push((file_path, e.to_string()));
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

    let filename = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let path_str = file_path.to_string_lossy();

    // Facet the file by its actual parent directory rather than the indexed
    // root, so `--filter <subdir>` matches every document beneath that
    // subdir. The search side canonicalizes the filter path; canonicalize
    // here too so the facet strings agree on symlinks and `.` components.
    let parent_dir = file_path
        .parent()
        .ok_or_else(|| Error::IndexedPathHasNoParent {
            path: file_path.to_path_buf(),
        })?;
    let canonical_parent = std::fs::canonicalize(parent_dir)
        .context(error::CanonicalizeSnafu { path: parent_dir })?;
    let directory_facet = path_to_facet(&canonical_parent);

    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let extension_facet = extension_to_facet(extension);

    // `path_exact` is the untokenized keyword copy; this delete actually
    // matches the previous chunks recorded for `file_path`. Deleting via
    // `schema.path` would silently no-op because that field is stemmed.
    writer.delete_term(Term::from_field_text(schema.path_exact, &path_str));

    for (offset, chunk) in chunk_content(&content) {
        writer
            .add_document(doc!(
                schema.path => path_str.as_ref(),
                schema.path_exact => path_str.as_ref(),
                schema.content => chunk,
                schema.filename => filename,
                schema.chunk_offset => offset as u64,
                schema.directory => directory_facet.clone(),
                schema.extension => extension_facet.clone(),
            ))
            .context(error::CreateIndexSnafu)?;
    }

    Ok(())
}
