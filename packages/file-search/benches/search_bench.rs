use file_search::EphemeralSearch;
use tango_bench::{IntoBenchmarks, benchmark_fn, tango_benchmarks, tango_main};

const SAMPLE_RUST_CODE: &str = r"
pub fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

pub struct SearchIndex {
    index: tantivy::Index,
    reader: tantivy::IndexReader,
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::open_or_create()
    }
}
";

const SAMPLE_QUERIES: &[&str] = &[
    "fibonacci",
    "pub fn",
    "SearchIndex",
    "tantivy",
    "impl",
    "fn new",
    "match n",
];

fn search_benchmarks() -> impl IntoBenchmarks {
    [
        benchmark_fn("ephemeral_search_simple_query", |b| {
            let search = EphemeralSearch::from_texts(std::iter::once(SAMPLE_RUST_CODE.to_string()))
                .expect("build ephemeral index");
            b.iter(move || search.search("fibonacci", 10).ok())
        }),
        benchmark_fn("ephemeral_search_multiple_queries", |b| {
            let search = EphemeralSearch::from_texts(std::iter::once(SAMPLE_RUST_CODE.to_string()))
                .expect("build ephemeral index");
            let mut query_idx = 0;
            b.iter(move || {
                let query = SAMPLE_QUERIES[query_idx % SAMPLE_QUERIES.len()];
                query_idx += 1;
                search.search(query, 10).ok()
            })
        }),
        benchmark_fn("ephemeral_search_create_single", |b| {
            b.iter(|| {
                EphemeralSearch::from_texts(std::iter::once(SAMPLE_RUST_CODE.to_string())).ok()
            })
        }),
        benchmark_fn("ephemeral_search_bulk_create_and_search", |b| {
            b.iter(|| {
                let texts = (0..10).map(|_| SAMPLE_RUST_CODE.to_string());
                let search = EphemeralSearch::from_texts(texts).expect("build ephemeral index");
                search.search("fibonacci", 10).ok()
            })
        }),
        benchmark_fn("ephemeral_search_large_text", |b| {
            let large_content = SAMPLE_RUST_CODE.repeat(100);
            b.iter(move || {
                let search = EphemeralSearch::from_texts(std::iter::once(large_content.clone()))
                    .expect("build ephemeral index");
                search.search("fibonacci", 10).ok()
            })
        }),
        benchmark_fn("ephemeral_search_many_small_texts", |b| {
            b.iter(|| {
                let texts = (0..50).map(|_| SAMPLE_RUST_CODE.to_string());
                let search = EphemeralSearch::from_texts(texts).expect("build ephemeral index");
                search.search("impl", 10).ok()
            })
        }),
    ]
}

tango_benchmarks!(search_benchmarks());
tango_main!();
