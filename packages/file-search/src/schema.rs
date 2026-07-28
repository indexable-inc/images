use code_tokenizer::CODE_STEMMED_TOKENIZER;
use tantivy::schema::{IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions};

pub fn build_schema() -> Schema {
    let text_indexing = TextFieldIndexing::default()
        .set_tokenizer(CODE_STEMMED_TOKENIZER)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);

    let text_options = TextOptions::default()
        .set_indexing_options(text_indexing)
        .set_stored();

    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("path", text_options.clone());
    // Untokenized keyword copy of the path so `delete_term` can match an
    // existing document by its exact path. The `path` field is stemmed and
    // would never round-trip a full path string as a single term.
    schema_builder.add_text_field("path_exact", STRING);
    schema_builder.add_text_field("content", text_options.clone());
    schema_builder.add_text_field("filename", text_options);
    schema_builder.add_u64_field("chunk_offset", STORED);
    // Directory and extension are stored as untokenized keyword strings
    // (rather than tantivy `Facet`s) so byte-range filters can match an
    // exact dir plus its descendants without auto-matching same-prefix
    // siblings (e.g. `/repo/src` vs `/repo/src-old`).
    schema_builder.add_text_field("directory", STRING);
    schema_builder.add_text_field("extension", STRING);
    schema_builder.build()
}
