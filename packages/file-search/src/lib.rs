//! BM25 file indexer and searcher built on Tantivy.
//!
//! [`SearchIndex`] owns a Tantivy index rooted at a caller-provided directory
//! and serves ranked searches with optional directory filters.
//! [`EphemeralSearch`] runs the same pipeline against a [`RamDirectory`] for
//! callers that just want to rerank a batch of texts in memory.
//!
//! [`RamDirectory`]: tantivy::directory::RamDirectory

pub mod ephemeral;
pub mod error;
mod indexing;
mod schema;
mod search;
mod types;

pub use ephemeral::{EphemeralSearch, RankResult};
pub use error::{Error, Result};
pub use repo_walker::{FileScanner, GitignoreFilter, WalkOptions, is_indexable_file};
pub use types::{IndexStats, SearchResult};

use snafu::ResultExt;
use std::path::{Path, PathBuf};
use tantivy::{Index, IndexReader, IndexWriter};
use types::IndexSchema;

const WRITER_HEAP_BYTES: usize = 50_000_000;

pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    schema: IndexSchema,
}

impl SearchIndex {
    /// Open an existing index in `index_dir`, or create a new one if it does
    /// not exist yet. The directory is created on demand.
    pub fn open_or_create(index_dir: impl Into<PathBuf>) -> Result<Self> {
        let index_dir = index_dir.into();
        let schema = schema::build_schema();
        let index_schema = IndexSchema::from_schema(&schema)?;

        let index = if index_dir.join("meta.json").exists() {
            Index::open_in_dir(&index_dir).context(error::OpenIndexSnafu { path: &index_dir })?
        } else {
            std::fs::create_dir_all(&index_dir).context(error::CreateIndexDirSnafu {
                path: index_dir.clone(),
            })?;
            Index::create_in_dir(&index_dir, schema).context(error::CreateIndexSnafu)?
        };

        code_tokenizer::register_tokenizers(&index);

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .context(error::CreateIndexWriterSnafu)?;

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context(error::CreateIndexSnafu)?;

        Ok(Self {
            index,
            reader,
            writer,
            schema: index_schema,
        })
    }

    /// Walk `directory`, indexing every file the scanner considers
    /// text-shaped. Honors `.gitignore` when `respect_gitignore` is true.
    pub fn index_directory(
        &mut self,
        directory: &Path,
        respect_gitignore: bool,
    ) -> Result<IndexStats> {
        indexing::index_directory(&mut self.writer, &self.schema, directory, respect_gitignore)
    }

    /// Search the index for the top `limit` hits matching `query`. When
    /// `filter_directory` is set, only documents whose canonicalized source
    /// directory equals it (or sits beneath it) are returned.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        filter_directory: Option<&Path>,
    ) -> Result<Vec<SearchResult>> {
        search::search(
            &self.index,
            &self.reader,
            &self.schema,
            query,
            limit,
            filter_directory,
        )
    }
}
