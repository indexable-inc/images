use crate::{
    error::{self, Result},
    indexing::directory_term,
    types::{IndexSchema, SearchResult},
};
use snafu::ResultExt;
use std::ops::Bound;
use std::path::Path;
use tantivy::{
    Index, IndexReader, Term,
    collector::TopDocs,
    query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery},
    schema::Value,
};

// Filename matches rank above raw path matches, which rank above content
// hits. A user typing `foo` usually wants the file named `foo.rs`, not every
// file that happens to mention `foo`.
const FILENAME_BOOST: f32 = 3.0;
const PATH_BOOST: f32 = 2.0;

pub fn search(
    index: &Index,
    reader: &IndexReader,
    schema: &IndexSchema,
    query: &str,
    limit: usize,
    filter_directory: Option<&Path>,
) -> Result<Vec<SearchResult>> {
    // Tantivy's `TopDocs::with_limit` asserts a nonzero limit; a zero limit
    // means "no hits", not a panic.
    if limit == 0 {
        return Ok(Vec::new());
    }

    reader.reload().context(error::SearchSnafu)?;
    let searcher = reader.searcher();

    let mut parser =
        QueryParser::for_index(index, vec![schema.content, schema.filename, schema.path]);
    parser.set_field_boost(schema.filename, FILENAME_BOOST);
    parser.set_field_boost(schema.path, PATH_BOOST);

    let content_query = parser.parse_query(query).context(error::QueryParseSnafu)?;

    let final_query: Box<dyn Query> = match filter_directory {
        Some(dir_path) => {
            let canonical_dir = std::fs::canonicalize(dir_path)
                .context(error::CanonicalizeSnafu { path: dir_path })?;

            // Files store their parent directory as `<canonical>/` (see
            // `indexing::directory_term`). The matching byte range is then
            // `[<dir>/, <dir>0)`: anything bytewise >= `<dir>/` and < the
            // next character after `/` (0x2F → 0x30 = '0'). This catches
            // the exact directory plus every descendant, without crossing
            // into same-prefix siblings (because '/' < '0' and '/' > '-',
            // so e.g. `<dir>-old/` falls below the lower bound).
            let lower_str = directory_term(&canonical_dir);
            let mut upper_str = lower_str.clone();
            upper_str.pop(); // strip the trailing '/'
            upper_str.push('0'); // next byte after '/' in ASCII

            let lower = Term::from_field_text(schema.directory, &lower_str);
            let upper = Term::from_field_text(schema.directory, &upper_str);
            let dir_range = RangeQuery::new(Bound::Included(lower), Bound::Excluded(upper));

            Box::new(BooleanQuery::new(vec![
                (Occur::Must, content_query),
                (Occur::Must, Box::new(dir_range)),
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
