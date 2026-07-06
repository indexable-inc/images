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
fn reindexing_removes_old_chunks() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let file = workdir.path().join("subject.md");
    fs::write(&file, "alpha bravo charlie").expect("write v1");

    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v1");
        let hits = index.search("alpha", 5, None).expect("search v1");
        assert!(!hits.is_empty(), "v1 should match `alpha`");
    }

    fs::write(&file, "delta echo foxtrot").expect("write v2");
    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v2");
        let alpha_hits = index.search("alpha", 5, None).expect("search alpha");
        assert!(
            alpha_hits.is_empty(),
            "stale chunk should be gone after re-index: {alpha_hits:?}",
        );
        let delta_hits = index.search("delta", 5, None).expect("search delta");
        assert!(!delta_hits.is_empty(), "v2 should match `delta`");
    }
}

#[test]
fn directory_filter_matches_subdirectory() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let inside = workdir.path().join("inside");
    let outside = workdir.path().join("outside");
    fs::create_dir(&inside).expect("mkdir inside");
    fs::create_dir(&outside).expect("mkdir outside");

    fs::write(inside.join("hit.rs"), "fn target() {}").expect("write inside");
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
fn reindex_removes_deleted_file_chunks() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let kept = workdir.path().join("kept.md");
    let removed = workdir.path().join("gone.md");
    fs::write(&kept, "alpha bravo").expect("write kept");
    fs::write(&removed, "alpha charlie").expect("write removed");

    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v1");
        let hits = index.search("charlie", 5, None).expect("search v1");
        assert!(!hits.is_empty(), "removed file should be searchable in v1");
    }

    fs::remove_file(&removed).expect("rm removed");
    {
        let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
        index
            .index_directory(workdir.path(), false)
            .expect("index v2");
        let hits = index.search("charlie", 5, None).expect("search v2");
        assert!(
            hits.is_empty(),
            "chunks for deleted file should be gone: {hits:?}",
        );
        let alpha_hits = index.search("alpha", 5, None).expect("search alpha");
        assert!(!alpha_hits.is_empty(), "surviving file should still match");
    }
}

#[test]
fn directory_filter_excludes_same_prefix_siblings() {
    let workdir = TempDir::new().expect("workdir");
    let index_dir = TempDir::new().expect("index dir");

    let src = workdir.path().join("src");
    let src_old = workdir.path().join("src-old");
    fs::create_dir(&src).expect("mkdir src");
    fs::create_dir(&src_old).expect("mkdir src-old");
    fs::write(src.join("kept.rs"), "fn target() {}").expect("write src");
    fs::write(src_old.join("dropped.rs"), "fn target() {}").expect("write src-old");

    let mut index = SearchIndex::open_or_create(index_dir.path()).expect("open");
    index.index_directory(workdir.path(), false).expect("index");

    let hits = index
        .search("target", 10, Some(src.as_path()))
        .expect("filter src");
    assert!(!hits.is_empty(), "filter for /src should match kept.rs");
    for hit in &hits {
        assert!(
            !hit.path.contains("src-old"),
            "filter for /src must not pull in /src-old: {hit:?}",
        );
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
