use file_search::{EphemeralSearch, SearchIndex};
use std::fs;
use tempfile::TempDir;

#[test]
fn index_then_search_finds_path_by_filename() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    fs::write(
        workdir.path().join("widgets.rs"),
        "pub fn make_widget() -> Widget { Widget }",
    )
    .expect("write source");
    fs::write(
        workdir.path().join("notes.md"),
        "Random documentation about thingamajigs.",
    )
    .expect("write notes");

    let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open index");
    let stats = index
        .index_directory(workdir.path(), false)
        .expect("index directory");
    assert_eq!(stats.files_indexed, 2, "{stats:?}");

    let hits = index.search("widgets", 5, None).expect("search");
    assert!(
        hits.iter().any(|h| h.path.ends_with("widgets.rs")),
        "filename should rank highest: {hits:?}",
    );
}

#[test]
fn ephemeral_reranks_matching_text_higher() {
    let search = EphemeralSearch::from_texts([
        "totally unrelated content".to_string(),
        "fibonacci runs in exponential time without memoization".to_string(),
        "another distractor entry".to_string(),
    ])
    .expect("build ephemeral");

    let results = search.search("fibonacci", 3).expect("search");
    let top = results.first().expect("at least one hit");
    assert_eq!(top.id, 1, "expected the fibonacci text to win: {results:?}");
}
