//! In-memory BM25 rerankers.
//!
//! [`EphemeralSearch`] builds a one-shot Tantivy index over an iterator of
//! texts and serves queries against it without touching the disk.
//! [`MultiFieldEphemeralSearch`] does the same for documents made of several
//! named, individually boosted fields.

use crate::error::{
    self, CommitIndexSnafu, CreateIndexSnafu, CreateIndexWriterSnafu, DocumentFieldCountSnafu,
    DuplicateFieldSnafu, NoFieldsSnafu, QueryParseSnafu, Result, SearchSnafu,
};
use snafu::{ResultExt, ensure};
use tantivy::{
    Index, IndexReader, TantivyDocument,
    collector::TopDocs,
    doc,
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
        let EphemeralSchema {
            schema,
            id_field,
            content_field,
        } = build_schema();

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
        // Tantivy's `TopDocs::with_limit` asserts a nonzero limit; a zero
        // limit means "no hits", not a panic. This also covers reranking an
        // empty batch, whose default limit is the batch size.
        if limit == 0 {
            return Ok(Vec::new());
        }

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

/// One field of a [`MultiFieldEphemeralSearch`]: the name it takes in the
/// in-memory schema, and the BM25 boost applied to matches inside it.
#[derive(Debug, Clone, Copy)]
pub struct FieldBoost {
    pub name: &'static str,
    pub boost: f32,
}

/// In-memory BM25 index over documents made of several named, boosted fields.
///
/// Same mechanism the on-disk index uses to rank a filename match above a
/// content match (`src/search.rs`), lifted to a caller-supplied field set.
///
/// The alternative is to keep [`EphemeralSearch`]'s single `content` field and
/// repeat a boosted field's text N times in it. That costs no new code but is
/// not the same thing: repetition inflates the document length, and BM25's
/// length normalization then discounts *every* term in that document,
/// including the ones the caller did not want to boost.
pub struct MultiFieldEphemeralSearch {
    index: Index,
    reader: IndexReader,
    id_field: Field,
    /// Schema field handles paired with their boosts, in the order the caller
    /// declared them. Document values are positional against this list.
    fields: Vec<(Field, f32)>,
}

impl MultiFieldEphemeralSearch {
    /// Build an in-memory index over `documents`, where each document carries
    /// one text value per entry of `fields`, in the same order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoFields`](crate::Error::NoFields) when `fields` is
    /// empty, [`Error::DuplicateField`](crate::Error::DuplicateField) when a
    /// name repeats, and
    /// [`Error::DocumentFieldCount`](crate::Error::DocumentFieldCount) when a
    /// document's value count does not match `fields`. Also returns an error
    /// if the index, writer, or reader cannot be created, or if a document
    /// cannot be added or committed.
    pub fn from_documents(
        fields: &[FieldBoost],
        documents: impl IntoIterator<Item = Vec<String>>,
    ) -> Result<Self> {
        ensure!(!fields.is_empty(), NoFieldsSnafu);
        for (position, spec) in fields.iter().enumerate() {
            // A repeated name would silently give two schema fields the same
            // label, and the caller could never tell which one it wrote to.
            ensure!(
                !fields[..position]
                    .iter()
                    .any(|prior| prior.name == spec.name),
                DuplicateFieldSnafu { name: spec.name }
            );
        }

        let mut builder = Schema::builder();
        let id_field = builder.add_u64_field("id", STORED);
        let mut boosted = Vec::with_capacity(fields.len());
        for spec in fields {
            let field = builder.add_text_field(spec.name, text_options());
            boosted.push((field, spec.boost));
        }

        let index = Index::builder()
            .schema(builder.build())
            .create_in_ram()
            .context(CreateIndexSnafu)?;

        code_tokenizer::register_tokenizers(&index);

        let mut writer = index
            .writer(crate::WRITER_HEAP_BYTES)
            .context(CreateIndexWriterSnafu)?;

        for (id, values) in documents.into_iter().enumerate() {
            ensure!(
                values.len() == boosted.len(),
                DocumentFieldCountSnafu {
                    id,
                    found: values.len(),
                    expected: boosted.len(),
                }
            );

            let mut document = TantivyDocument::default();
            document.add_u64(id_field, id as u64);
            for ((field, _), value) in boosted.iter().zip(values) {
                document.add_text(*field, value);
            }
            writer.add_document(document).context(CreateIndexSnafu)?;
        }

        writer.commit().context(CommitIndexSnafu)?;
        // See `EphemeralSearch::from_texts`: the reader holds the committed
        // segments, and the RamDirectory belongs to the Index, so dropping the
        // writer here is safe.
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
            fields: boosted,
        })
    }

    /// Return up to `limit` hits ranked by boosted BM25, with each
    /// [`RankResult::id`] referencing the document's position in the iterator
    /// passed to [`Self::from_documents`].
    ///
    /// # Errors
    ///
    /// Returns an error if the query cannot be parsed or the search fails.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<RankResult>> {
        // `TopDocs::with_limit` asserts a nonzero limit; a zero limit means
        // "no hits", not a panic.
        if limit == 0 {
            return Ok(Vec::new());
        }

        let mut parser = QueryParser::for_index(
            &self.index,
            self.fields.iter().map(|&(field, _)| field).collect(),
        );
        for &(field, boost) in &self.fields {
            parser.set_field_boost(field, boost);
        }
        let parsed = parser.parse_query(query).context(QueryParseSnafu)?;

        let searcher = self.reader.searcher();
        let top_docs = searcher
            .search(&parsed, &TopDocs::with_limit(limit).order_by_score())
            .context(SearchSnafu)?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address).context(error::SearchSnafu)?;
            results.push(RankResult {
                id: stored_id(&doc, self.id_field),
                score,
            });
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

/// Indexing options shared by every ephemeral text field: the code-aware
/// stemmed tokenizer, with freqs and positions so phrase queries work.
fn text_options() -> TextOptions {
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer(code_tokenizer::CODE_STEMMED_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);

    TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored()
}

fn build_schema() -> EphemeralSchema {
    let mut builder = Schema::builder();
    let id_field = builder.add_u64_field("id", STORED);
    let content_field = builder.add_text_field("content", text_options());
    EphemeralSchema {
        schema: builder.build(),
        id_field,
        content_field,
    }
}

/// Read back the `id` a document was written with. The id was assigned by
/// `enumerate()`, which yields `usize`, so on the 64-bit targets we support
/// this narrowing is lossless.
#[expect(
    clippy::cast_possible_truncation,
    reason = "id originated as usize on the 64-bit targets we support"
)]
fn stored_id(doc: &TantivyDocument, id_field: Field) -> usize {
    doc.get_first(id_field)
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use super::{FieldBoost, MultiFieldEphemeralSearch};
    use crate::error::{Error, Result};

    /// `expect_err` needs the `Ok` type to be `Debug`, and a live Tantivy
    /// index is not; unwrap the error by hand instead of deriving `Debug` on
    /// a handle that holds a reader.
    fn expect_error(result: Result<MultiFieldEphemeralSearch>, why: &str) -> Error {
        match result {
            Ok(_) => panic!("{why}"),
            Err(error) => error,
        }
    }

    /// Two documents that are mirror images of each other: the query term sits
    /// in `tldr` for one and in `body` for the other, and each field holds the
    /// same two documents' worth of text. Every BM25 input is therefore
    /// symmetric and the boost is the only thing that can order them, so this
    /// asserts the boost and nothing else.
    fn mirrored_documents() -> Vec<Vec<String>> {
        vec![
            vec!["nix rebuild".to_owned(), "filler filler".to_owned()],
            vec!["filler filler".to_owned(), "nix rebuild".to_owned()],
        ]
    }

    #[test]
    fn boosted_field_outranks_unboosted_field() {
        let fields = [
            FieldBoost {
                name: "tldr",
                boost: 3.0,
            },
            FieldBoost {
                name: "body",
                boost: 1.0,
            },
        ];
        let index = MultiFieldEphemeralSearch::from_documents(&fields, mirrored_documents())
            .expect("building a two-document in-RAM index");
        let hits = index.search("nix", 10).expect("searching for nix");

        assert_eq!(hits.len(), 2, "both documents match: {hits:?}");
        assert_eq!(hits[0].id, 0, "the tldr match must rank first: {hits:?}");
        assert!(
            hits[0].score > hits[1].score,
            "boosted hit must score strictly higher: {hits:?}"
        );
    }

    #[test]
    fn swapping_the_boost_swaps_the_order() {
        let fields = [
            FieldBoost {
                name: "tldr",
                boost: 1.0,
            },
            FieldBoost {
                name: "body",
                boost: 3.0,
            },
        ];
        let index = MultiFieldEphemeralSearch::from_documents(&fields, mirrored_documents())
            .expect("building a two-document in-RAM index");
        let hits = index.search("nix", 10).expect("searching for nix");

        assert_eq!(hits.len(), 2, "both documents match: {hits:?}");
        assert_eq!(
            hits[0].id, 1,
            "the body match must now rank first: {hits:?}"
        );
    }

    #[test]
    fn document_with_wrong_field_count_is_an_error() {
        let fields = [
            FieldBoost {
                name: "tldr",
                boost: 3.0,
            },
            FieldBoost {
                name: "body",
                boost: 1.0,
            },
        ];
        let documents = vec![vec!["only one value".to_owned()]];
        let error = expect_error(
            MultiFieldEphemeralSearch::from_documents(&fields, documents),
            "a short document must not be silently padded",
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains("carries 1 field values"),
            "message should name the mismatch, got {rendered}"
        );
    }

    #[test]
    fn empty_field_set_is_an_error() {
        let error = expect_error(
            MultiFieldEphemeralSearch::from_documents(&[], Vec::new()),
            "an index with no fields can never match",
        );
        assert!(
            error.to_string().contains("at least one field"),
            "got {error}"
        );
    }

    #[test]
    fn duplicate_field_name_is_an_error() {
        let fields = [
            FieldBoost {
                name: "tldr",
                boost: 3.0,
            },
            FieldBoost {
                name: "tldr",
                boost: 1.0,
            },
        ];
        let error = expect_error(
            MultiFieldEphemeralSearch::from_documents(&fields, Vec::new()),
            "two fields cannot share a name",
        );
        assert!(error.to_string().contains("declared twice"), "got {error}");
    }
}
