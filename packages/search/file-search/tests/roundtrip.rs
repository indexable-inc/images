use file_search::{EphemeralSearch, SearchIndex, SearchIndexReader};
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
fn directory_filter_is_path_aware() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let inside = workdir.path().join("inside");
    let same_prefix = workdir.path().join("inside-old");
    let outside = workdir.path().join("outside");
    fs::create_dir(&inside).expect("mkdir inside");
    fs::create_dir(&same_prefix).expect("mkdir same-prefix sibling");
    fs::create_dir(&outside).expect("mkdir outside");

    fs::write(inside.join("hit.rs"), "fn target() {}").expect("write inside");
    fs::write(same_prefix.join("prefix-miss.rs"), "fn target() {}")
        .expect("write same-prefix sibling");
    fs::write(outside.join("miss.rs"), "fn target() {}").expect("write outside");

    let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
    index.index_directory(workdir.path(), false).expect("index");

    let hits = index
        .search("target", 10, Some(inside.as_path()))
        .expect("search filtered");
    assert!(
        !hits.is_empty(),
        "subdirectory filter should match indexed files"
    );
    for hit in &hits {
        assert!(
            hit.path.contains("/inside/"),
            "filtered hit escaped subdir: {hit:?}",
        );
    }
}

#[test]
fn reindex_removes_stale_and_deleted_file_chunks() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let kept = workdir.path().join("kept.md");
    let removed = workdir.path().join("gone.md");
    fs::write(&kept, "alpha bravo").expect("write kept v1");
    fs::write(&removed, "charlie delta").expect("write removed");

    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v1");
        assert!(!index.search("alpha", 5, None).expect("search kept v1").is_empty());
        assert!(!index.search("charlie", 5, None).expect("search removed v1").is_empty());
    }

    fs::write(&kept, "echo foxtrot").expect("write kept v2");
    fs::remove_file(&removed).expect("rm removed");
    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v2");
        assert!(index.search("alpha", 5, None).expect("stale search").is_empty());
        assert!(index.search("charlie", 5, None).expect("deleted search").is_empty());
        assert!(!index.search("foxtrot", 5, None).expect("current search").is_empty());
    }
}

#[test]
fn search_index_reader_opens_without_writer_lock() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    fs::write(workdir.path().join("note.md"), "indexable content here").expect("write");

    // Keep the writer alive in the indexer; a second SearchIndex would
    // block on the writer lock, but SearchIndexReader should not.
    let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open writer");
    index.index_directory(workdir.path(), false).expect("index");

    let reader = SearchIndexReader::open(index_dir.path()).expect("open reader concurrently");
    let hits = reader.search("indexable", 5, None).expect("reader search");
    assert!(!hits.is_empty(), "reader should see committed docs");
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

#[test]
fn search_limit_zero_returns_empty() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    fs::write(workdir.path().join("note.md"), "alpha bravo charlie").expect("write");

    let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
    index.index_directory(workdir.path(), false).expect("index");

    // Tantivy's `TopDocs::with_limit(0)` panics; a zero limit must instead
    // return no hits.
    let hits = index.search("alpha", 0, None).expect("limit 0 search");
    assert!(hits.is_empty(), "limit 0 should return no hits: {hits:?}");
}

#[test]
fn ephemeral_limit_zero_returns_empty() {
    let search =
        EphemeralSearch::from_texts(["alpha bravo charlie".to_string()]).expect("build ephemeral");

    // Tantivy's `TopDocs::with_limit(0)` panics; a zero limit must instead
    // return no hits. Reranking an empty batch defaults to this limit.
    let hits = search.search("alpha", 0).expect("limit 0 search");
    assert!(hits.is_empty(), "limit 0 should return no hits: {hits:?}");
}
