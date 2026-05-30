//! Search orchestration. The backend stores one shared entry per unique blob,
//! so a raw result set can include content from other worktrees or branches.
//! We over-fetch, then keep only hits whose content hash is in this checkout's
//! manifest, mapping each back to its local path. Web results (no hash) pass
//! through when the caller asked for them.

use crate::backend::{SearchHit, SearchOptions, Store};
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
pub async fn search(
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
    use super::search;
    use crate::backend::{MemoryStore, SearchOptions, Store, UploadMeta};
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
        let hits = search(&store, "s", &manifest, "needle", 10, opts(), false)
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

        let hits = search(&store, "s", &manifest, "needle", 10, opts(), false)
            .await
            .expect("search");
        assert!(hits.is_empty());
    }
}
