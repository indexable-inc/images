use crate::{
    error::{self, Result},
    types::{IndexSchema, SearchResult},
};
use snafu::ResultExt;
use std::ops::Bound;
use std::path::Path;
use tantivy::{
    Index, IndexReader, Term,
    collector::TopDocs,
    query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery},
    schema::{Facet, Value},
};

// Filename matches rank above raw path matches, which rank above content
// hits. A user typing `foo` usually wants the file named `foo.rs`, not every
// file that happens to mention `foo`.
const FILENAME_BOOST: f32 = 3.0;
const PATH_BOOST: f32 = 2.0;

fn path_to_facet(path: &Path) -> Facet {
    let path_str = path.to_string_lossy();
    let normalized = if path_str.starts_with('/') {
        path_str.into_owned()
    } else {
        format!("/{path_str}")
    };
    Facet::from(&normalized)
}

pub fn search(
    index: &Index,
    reader: &IndexReader,
    schema: &IndexSchema,
    query: &str,
    limit: usize,
    filter_directory: Option<&Path>,
) -> Result<Vec<SearchResult>> {
    reader.reload().context(error::SearchSnafu)?;
    let searcher = reader.searcher();

    let mut parser = QueryParser::for_index(index, vec![schema.content, schema.filename, schema.path]);
    parser.set_field_boost(schema.filename, FILENAME_BOOST);
    parser.set_field_boost(schema.path, PATH_BOOST);

    let content_query = parser.parse_query(query).context(error::QueryParseSnafu)?;

    let final_query: Box<dyn Query> = match filter_directory {
        Some(dir_path) => {
            let canonical_dir = std::fs::canonicalize(dir_path)
                .context(error::CanonicalizeSnafu { path: dir_path })?;
            let dir_facet = path_to_facet(&canonical_dir);

            let lower_bound = Term::from_facet(schema.directory, &dir_facet);

            // Tantivy ranges over facets work on the encoded prefix; appending
            // U+FFFF gives us "everything that starts with this facet" without
            // matching unrelated facets that happen to share a prefix.
            let mut upper_encoded = dir_facet.encoded_str().to_string();
            upper_encoded.push('\u{FFFF}');
            let upper_bound = Term::from_field_bytes(schema.directory, upper_encoded.as_bytes());

            let facet_range = RangeQuery::new(
                Bound::Included(lower_bound),
                Bound::Excluded(upper_bound),
            );

            Box::new(BooleanQuery::new(vec![
                (Occur::Must, content_query),
                (Occur::Must, Box::new(facet_range)),
            ]))
        }
        None => content_query,
    };

    let top_docs = searcher
        .search(&*final_query, &TopDocs::with_limit(limit).order_by_score())
        .context(error::SearchSnafu)?;

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in top_docs {
        let doc: tantivy::TantivyDocument =
            searcher.doc(doc_address).context(error::SearchSnafu)?;

        let path = doc
            .get_first(schema.path)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let snippet = doc
            .get_first(schema.content)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chunk_offset = doc
            .get_first(schema.chunk_offset)
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        results.push(SearchResult {
            path,
            score,
            snippet,
            chunk_offset,
        });
    }

    Ok(results)
}
