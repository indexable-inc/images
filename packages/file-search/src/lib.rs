//! BM25 file indexer and searcher built on Tantivy.
//!
//! [`SearchIndexReader`] opens an existing index for read-only search.
//! [`SearchIndex`] additionally holds a writer for indexing new files; it
//! takes Tantivy's per-directory writer lock, so callers that only need to
//! search a shared or read-only index should use [`SearchIndexReader`].
//!
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
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy};
use types::IndexSchema;

const WRITER_HEAP_BYTES: usize = 50_000_000;

/// Read-only view over an on-disk Tantivy index. Does not take the writer
/// lock, so multiple readers can run concurrently with at most one
/// [`SearchIndex`].
pub struct SearchIndexReader {
    index: Index,
    reader: IndexReader,
    schema: IndexSchema,
}

impl SearchIndexReader {
    /// Open an existing index at `index_dir` for searching.
    ///
    /// # Errors
    ///
    /// Returns an error if `index_dir` does not contain a valid Tantivy
    /// index, if the schema is missing a field the reader expects, or if
    /// the reader cannot be initialized. Use [`SearchIndex::open_or_create`]
    /// when no index exists yet.
    pub fn open(index_dir: impl AsRef<Path>) -> Result<Self> {
        let index_dir = index_dir.as_ref();
        let index = Index::open_in_dir(index_dir).context(error::OpenIndexSnafu {
            path: index_dir.to_path_buf(),
        })?;
        code_tokenizer::register_tokenizers(&index);

        let schema = IndexSchema::from_schema(&index.schema())?;
        let reader = build_reader(&index)?;

        Ok(Self {
            index,
            reader,
            schema,
        })
    }

    /// Search the index for the top `limit` hits matching `query`. When
    /// `filter_directory` is set, only documents whose canonicalized parent
    /// directory equals it or sits beneath it are returned.
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be parsed, the search fails, or
    /// (when `filter_directory` is set) the filter path cannot be
    /// canonicalized.
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

/// Read-write handle to a Tantivy index. Owns a writer (and therefore the
/// directory's writer lock) so it can index new files; for search-only
/// access prefer [`SearchIndexReader`].
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    writer: IndexWriter,
    schema: IndexSchema,
}

impl SearchIndex {
    /// Open an existing index in `index_dir`, or create a new one if it does
    /// not exist yet. The directory is created on demand.
    ///
    /// # Errors
    ///
    /// Returns an error if `index_dir` cannot be created, if an existing
    /// index cannot be opened or has a mismatched schema, or if Tantivy
    /// cannot acquire the writer lock or initialize the reader.
    pub fn open_or_create(index_dir: impl Into<PathBuf>) -> Result<Self> {
        let index_dir = index_dir.into();

        let index = if index_dir.join("meta.json").exists() {
            Index::open_in_dir(&index_dir).context(error::OpenIndexSnafu { path: &index_dir })?
        } else {
            std::fs::create_dir_all(&index_dir).context(error::CreateIndexDirSnafu {
                path: index_dir.clone(),
            })?;
            Index::create_in_dir(&index_dir, schema::build_schema())
                .context(error::CreateIndexSnafu)?
        };

        // Derive `IndexSchema` from the opened index rather than the freshly
        // built schema. The on-disk index could have been created by an older
        // build whose field order differs from `build_schema`; using the
        // wrong field ids would corrupt reads. `from_schema` returns a
        // typed error if the index doesn't carry the fields we expect.
        let index_schema = IndexSchema::from_schema(&index.schema())?;
        code_tokenizer::register_tokenizers(&index);

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .context(error::CreateIndexWriterSnafu)?;
        let reader = build_reader(&index)?;

        Ok(Self {
            index,
            reader,
            writer,
            schema: index_schema,
        })
    }

    /// Walk `directory`, indexing every file the scanner considers
    /// text-shaped. Honors `.gitignore` when `respect_gitignore` is true.
    /// Per-file errors are recorded in [`IndexStats::errors`] instead of
    /// aborting the walk.
    ///
    /// # Errors
    ///
    /// Returns an error if the final commit fails. Individual files that
    /// fail to read, parse, or index are recorded in the returned
    /// [`IndexStats`] but do not abort the run.
    pub fn index_directory(
        &mut self,
        directory: &Path,
        respect_gitignore: bool,
    ) -> Result<IndexStats> {
        indexing::index_directory(&mut self.writer, &self.schema, directory, respect_gitignore)
    }

    /// Search the index for the top `limit` hits matching `query`. Same
    /// behavior as [`SearchIndexReader::search`].
    ///
    /// # Errors
    ///
    /// Same conditions as [`SearchIndexReader::search`].
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

fn build_reader(index: &Index) -> Result<IndexReader> {
    index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()
        .context(error::CreateIndexSnafu)
}
