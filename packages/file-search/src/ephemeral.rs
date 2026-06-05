//! In-memory BM25 reranker. [`EphemeralSearch`] builds a one-shot Tantivy
//! index over an iterator of texts and serves queries against it without
//! touching the disk.

use crate::error::{
    self, CommitIndexSnafu, CreateIndexSnafu, CreateIndexWriterSnafu, QueryParseSnafu, Result,
    SearchSnafu,
};
use snafu::ResultExt;
use tantivy::{
    Index, IndexReader, TantivyDocument, doc,
    collector::TopDocs,
    query::QueryParser,
    schema::{Field, IndexRecordOption, STORED, Schema, TextFieldIndexing, TextOptions, Value},
};

#[derive(Debug, Clone, Copy)]
pub struct RankResult {
    pub id: usize,
    pub score: f32,
}

pub struct EphemeralSearch {
    index: Index,
    reader: IndexReader,
    id_field: Field,
    content_field: Field,
}

impl EphemeralSearch {
    /// Build an in-memory index over `texts` and return a handle that can
    /// rerank them by BM25 score.
    ///
    /// # Errors
    ///
    /// Returns an error if the index, writer, or reader cannot be created,
    /// or if a document cannot be added or committed.
    pub fn from_texts(texts: impl IntoIterator<Item = String>) -> Result<Self> {
        let EphemeralSchema { schema, id_field, content_field } = build_schema();

        let index = Index::builder()
            .schema(schema)
            .create_in_ram()
            .context(CreateIndexSnafu)?;

        code_tokenizer::register_tokenizers(&index);

        let mut writer = index.writer(50_000_000).context(CreateIndexWriterSnafu)?;

        for (idx, text) in texts.into_iter().enumerate() {
            writer
                .add_document(doc!(
                    id_field => idx as u64,
                    content_field => text,
                ))
                .context(CreateIndexSnafu)?;
        }

        writer.commit().context(CommitIndexSnafu)?;
        // After commit the reader holds the live segments; dropping the writer
        // is safe because the RamDirectory is owned by the Index itself.
        drop(writer);

        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context(CreateIndexSnafu)?;

        Ok(Self {
            index,
            reader,
            id_field,
            content_field,
        })
    }

    /// Return up to `limit` hits ranked by BM25, with each [`RankResult::id`]
    /// referencing the position of the text in the iterator passed to
    /// [`Self::from_texts`].
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be parsed or the search fails.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<RankResult>> {
        let parser = QueryParser::for_index(&self.index, vec![self.content_field]);
        let parsed = parser.parse_query(query).context(QueryParseSnafu)?;

        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
            .context(SearchSnafu)?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).context(error::SearchSnafu)?;
            let raw_id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            // The id was assigned by `enumerate()`, which yields `usize`, so on
            // the 64-bit targets we support this widening cast is lossless (a
            // `u64` index id always fits in a 64-bit `usize`).
            #[expect(
                clippy::cast_possible_truncation,
                reason = "id originated as usize on the 64-bit targets we support"
            )]
            let id = raw_id as usize;

            results.push(RankResult { id, score });
        }

        Ok(results)
    }
}

/// The ephemeral index schema together with handles to its two fields.
struct EphemeralSchema {
    schema: Schema,
    id_field: Field,
    content_field: Field,
}

fn build_schema() -> EphemeralSchema {
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer(code_tokenizer::CODE_STEMMED_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);

    let text_options = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();

    let mut builder = Schema::builder();
    let id_field = builder.add_u64_field("id", STORED);
    let content_field = builder.add_text_field("content", text_options);
    EphemeralSchema {
        schema: builder.build(),
        id_field,
        content_field,
    }
}
