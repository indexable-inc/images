use code_tokenizer::CODE_STEMMED_TOKENIZER;
use tantivy::schema::{
    FacetOptions, IndexRecordOption, STRING, STORED, Schema, TextFieldIndexing, TextOptions,
};

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
    schema_builder.add_facet_field("directory", FacetOptions::default());
    schema_builder.add_facet_field("extension", FacetOptions::default());
    schema_builder.build()
}
