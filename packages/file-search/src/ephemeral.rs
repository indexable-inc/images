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
    pub fn from_texts(texts: impl IntoIterator<Item = String>) -> Result<Self> {
        let (schema, id_field, content_field) = build_schema();

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
            let id = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            results.push(RankResult { id, score });
        }

        Ok(results)
    }
}

fn build_schema() -> (Schema, Field, Field) {
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer(code_tokenizer::CODE_STEMMED_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);

    let text_options = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();

    let mut builder = Schema::builder();
    let id_field = builder.add_u64_field("id", STORED);
    let content_field = builder.add_text_field("content", text_options);
    (builder.build(), id_field, content_field)
}
