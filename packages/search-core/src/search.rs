//! Search orchestration. The backend stores one shared entry per unique blob,
//! so a raw result set can include content from other worktrees or branches.
//! We over-fetch, then keep only hits whose content hash is in this checkout's
//! manifest, mapping each back to its local path. Web results (no hash) pass
//! through when the caller asked for them.

use crate::backend::{GrepOptions, SearchHit, SearchOptions, Store};
use crate::config::WEB_STORE;
use crate::error::Result;
use crate::manifest::Manifest;

/// A search result projected onto the current checkout.
#[derive(Debug, Clone)]
pub struct DisplayHit {
    /// Repo-relative path, or a URL for a web result.
    pub label: String,
    /// Zero-based start line within the file, when known.
    pub start_line: Option<u32>,
    /// Number of lines in the chunk (a count, not a span), when known.
    pub num_lines: Option<u32>,
    /// Relevance score in `0.0..=1.0`.
    pub score: f32,
    /// Matched snippet text.
    pub text: String,
    /// Whether this came from the web store rather than the local checkout.
    pub is_web: bool,
}

/// A question-answering result projected onto the current checkout.
#[derive(Debug, Clone)]
pub struct AnswerView {
    /// The synthesized answer.
    pub answer: String,
    /// Sources, filtered and mapped like search hits.
    pub sources: Vec<DisplayHit>,
}

/// Search `store_name` (and optionally the web store) and return only hits
/// present in this checkout, newest-relevance first, capped at `top_k`.
///
/// # Errors
/// Returns an error if the backend search request fails.
pub async fn semantic(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    query: &str,
    top_k: usize,
    options: SearchOptions,
    include_web: bool,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, include_web);
    let hits = store
        .search(&stores, query, overfetch(top_k), options)
        .await?;
    Ok(project(manifest, hits, include_web, top_k))
}

/// Grep `store_name` with a regular expression, scoped to this checkout.
///
/// Runs the pattern over the same chunks [`semantic`] searches, returning only
/// hits present in this checkout, capped at `top_k`. Grep is local-corpus only:
/// the web store is never queried, so the projection runs with `include_web`
/// false.
///
/// # Errors
/// Returns an error if the pattern is not a valid regular expression or the
/// backend grep request fails.
pub async fn grep(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    pattern: &str,
    top_k: usize,
    options: GrepOptions,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, false);
    let hits = store
        .grep(&stores, pattern, overfetch(top_k), options)
        .await?;
    Ok(project(manifest, hits, false, top_k))
}

/// Ask a question against `store_name` (and optionally the web store).
///
/// # Errors
/// Returns an error if the backend request fails.
pub async fn ask(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    query: &str,
    top_k: usize,
    options: SearchOptions,
    include_web: bool,
) -> Result<AnswerView> {
    let stores = store_identifiers(store_name, include_web);
    let answer = store.ask(&stores, query, overfetch(top_k), options).await?;
    Ok(AnswerView {
        answer: answer.answer,
        sources: project(manifest, answer.sources, include_web, top_k),
    })
}

fn store_identifiers(store_name: &str, include_web: bool) -> Vec<String> {
    let mut stores = vec![store_name.to_owned()];
    if include_web {
        stores.push(WEB_STORE.to_owned());
    }
    stores
}

/// Over-fetch so that client-side filtering still leaves enough results. Other
/// checkouts' content can crowd the raw top-k, so we ask for more than we show.
fn overfetch(top_k: usize) -> usize {
    top_k.saturating_mul(4).max(top_k.saturating_add(10))
}

fn project(
    manifest: &Manifest,
    hits: Vec<SearchHit>,
    include_web: bool,
    top_k: usize,
) -> Vec<DisplayHit> {
    let local = manifest.hashes();
    let mut out = Vec::with_capacity(top_k);
    for hit in hits {
        if out.len() >= top_k {
            break;
        }
        match hit.hash.as_deref() {
            Some(hash) if local.contains(hash) => {
                let label = manifest.path_for_hash(hash).unwrap_or(hash).to_owned();
                out.push(DisplayHit {
                    label,
                    start_line: hit.start_line,
                    num_lines: hit.num_lines,
                    score: hit.score,
                    text: hit.text,
                    is_web: false,
                });
            }
            None if include_web => out.push(DisplayHit {
                label: hit.path.unwrap_or_else(|| "(web)".to_owned()),
                start_line: None,
                num_lines: None,
                score: hit.score,
                text: hit.text,
                is_web: true,
            }),
            // A hash absent from this checkout belongs to another worktree or
            // branch; a non-web result with no hash is dropped too. Either way
            // search reflects only the current tree.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{grep, semantic};
    use crate::backend::{
        GrepOptions, GrepTargets, MemoryStore, SearchOptions, Store, UploadMeta,
    };
    use crate::content::ContentHash;
    use crate::manifest::{FileEntry, Manifest};

    fn opts() -> SearchOptions {
        SearchOptions {
            rerank: true,
            agentic: false,
        }
    }

    async fn put(store: &MemoryStore, path: &str, content: &str) -> ContentHash {
        let hash = ContentHash::of_bytes(content.as_bytes());
        store
            .upload(
                "s",
                content.as_bytes().to_vec(),
                path,
                hash.as_str(),
                UploadMeta {
                    path: path.to_owned(),
                    hash: hash.as_str().to_owned(),
                },
            )
            .await
            .expect("upload");
        hash
    }

    fn manifest_with(entries: &[(&str, &ContentHash)]) -> Manifest {
        Manifest {
            entries: entries
                .iter()
                .map(|(path, hash)| FileEntry {
                    rel_path: (*path).to_owned(),
                    hash: (*hash).clone(),
                    mtime_ms: 0,
                    size: 0,
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn results_outside_this_checkout_are_filtered() {
        let store = MemoryStore::new();
        let mine = put(&store, "mine.rs", "needle in mine").await;
        let _theirs = put(&store, "theirs.rs", "needle in theirs").await;

        // Manifest only knows about `mine.rs`, so the other worktree's hit must
        // be dropped even though both match the query in the shared store.
        let manifest = manifest_with(&[("mine.rs", &mine)]);
        let hits = semantic(&store, "s", &manifest, "needle", 10, opts(), false)
            .await
            .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "mine.rs");
        assert!(!hits[0].is_web);
    }

    #[tokio::test]
    async fn empty_manifest_yields_no_local_hits() {
        let store = MemoryStore::new();
        let _ = put(&store, "a.rs", "needle").await;
        let manifest = Manifest::default();

        let hits = semantic(&store, "s", &manifest, "needle", 10, opts(), false)
            .await
            .expect("search");
        assert!(hits.is_empty());
    }

    fn grep_opts(case_sensitive: bool) -> GrepOptions {
        GrepOptions {
            case_sensitive,
            targets: GrepTargets::Text,
        }
    }

    #[tokio::test]
    async fn grep_matches_regex_and_projects_to_local_paths() {
        let store = MemoryStore::new();
        let alpha = put(&store, "alpha.rs", "fn handler() {}\nlet other = 1;").await;
        let beta = put(&store, "beta.rs", "struct Thing;\nfn render() {}").await;

        let manifest = manifest_with(&[("alpha.rs", &alpha), ("beta.rs", &beta)]);
        let hits = grep(&store, "s", &manifest, r"fn \w+\(\)", 10, grep_opts(false))
            .await
            .expect("grep");

        // The regex matches the two `fn name()` lines, one per file, and each
        // hit projects back to its local repo-relative path.
        assert_eq!(hits.len(), 2);
        let mut labels: Vec<&str> = hits.iter().map(|hit| hit.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, vec!["alpha.rs", "beta.rs"]);
        assert!(hits.iter().all(|hit| !hit.is_web));
    }

    #[tokio::test]
    async fn grep_case_sensitive_excludes_differently_cased_match() {
        let store = MemoryStore::new();
        let lower = put(&store, "lower.rs", "let token = read();").await;
        let upper = put(&store, "upper.rs", "let TOKEN = read();").await;

        let manifest = manifest_with(&[("lower.rs", &lower), ("upper.rs", &upper)]);

        // Case-insensitive grep catches both spellings of the word.
        let insensitive = grep(&store, "s", &manifest, "token", 10, grep_opts(false))
            .await
            .expect("grep");
        assert_eq!(insensitive.len(), 2);

        // Case-sensitive grep keeps only the lowercase line.
        let sensitive = grep(&store, "s", &manifest, "token", 10, grep_opts(true))
            .await
            .expect("grep");
        assert_eq!(sensitive.len(), 1);
        assert_eq!(sensitive[0].label, "lower.rs");
    }

    #[tokio::test]
    async fn grep_respects_top_k() {
        let store = MemoryStore::new();
        let many = put(&store, "many.rs", "fn a() {}\nfn b() {}\nfn c() {}").await;
        let manifest = manifest_with(&[("many.rs", &many)]);

        let hits = grep(&store, "s", &manifest, r"fn \w", 2, grep_opts(false))
            .await
            .expect("grep");
        assert_eq!(hits.len(), 2);
    }

    #[tokio::test]
    async fn grep_invalid_pattern_is_an_error() {
        let store = MemoryStore::new();
        let only = put(&store, "only.rs", "anything").await;
        let manifest = manifest_with(&[("only.rs", &only)]);

        // An unbalanced group is not a valid regex, so grep must surface a typed
        // error rather than panic.
        let result = grep(&store, "s", &manifest, "fn (", 10, grep_opts(false)).await;
        assert!(result.is_err());
    }
}
