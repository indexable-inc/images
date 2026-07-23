use crate::error::{Result, SchemaMissingFieldSnafu};
use snafu::ResultExt;
use std::path::PathBuf;
use tantivy::schema::{Field, Schema};

pub struct IndexSchema {
    pub path: Field,
    pub path_exact: Field,
    pub content: Field,
    pub filename: Field,
    pub chunk_offset: Field,
    pub directory: Field,
    pub extension: Field,
}

impl IndexSchema {
    pub fn from_schema(schema: &Schema) -> Result<Self> {
        let field = |name: &'static str| {
            schema
                .get_field(name)
                .context(SchemaMissingFieldSnafu { field: name })
        };

        Ok(Self {
            path: field("path")?,
            path_exact: field("path_exact")?,
            content: field("content")?,
            filename: field("filename")?,
            chunk_offset: field("chunk_offset")?,
            directory: field("directory")?,
            extension: field("extension")?,
        })
    }
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub files_skipped: usize,
    pub errors: Vec<(PathBuf, String)>,
}

#[derive(Debug)]
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    pub snippet: String,
    pub chunk_offset: u64,
}
