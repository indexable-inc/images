use snafu::Snafu;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display("Failed to open Tantivy directory {path}: {source}", path = path.display()))]
    OpenIndex {
        path: PathBuf,
        source: tantivy::TantivyError,
    },

    #[snafu(display("Failed to create index directory {path}: {source}", path = path.display()))]
    CreateIndexDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to create Tantivy index: {source}"))]
    CreateIndex { source: tantivy::TantivyError },

    #[snafu(display("Failed to create index writer: {source}"))]
    CreateIndexWriter { source: tantivy::TantivyError },

    #[snafu(display("Failed to commit index: {source}"))]
    CommitIndex { source: tantivy::TantivyError },

    #[snafu(display("Schema is missing required field {field}: {source}"))]
    SchemaMissingField {
        field: &'static str,
        source: tantivy::TantivyError,
    },

    #[snafu(display("Failed to read file {path}: {source}", path = path.display()))]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read metadata for {path}: {source}", path = path.display()))]
    GetMetadata {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to canonicalize {path}: {source}", path = path.display()))]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display(
        "File too large: {path} ({size} bytes, max {max_size} bytes)",
        path = path.display(),
    ))]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        max_size: u64,
    },

    #[snafu(display("Indexed file {path} has no parent directory", path = path.display()))]
    IndexedPathHasNoParent { path: PathBuf },

    #[snafu(display(
        "Walker failed under {directory}: {source}",
        directory = directory.display(),
    ))]
    Walk {
        directory: PathBuf,
        source: repo_walker::WalkError,
    },

    #[snafu(display("Failed to search index: {source}"))]
    Search { source: tantivy::TantivyError },

    #[snafu(display("Failed to parse query: {source}"))]
    QueryParse {
        source: tantivy::query::QueryParserError,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
