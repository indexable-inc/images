//! Search orchestration and projection onto displayable hits.
//!
//! The backend holds one shared entry per unique record across every source. A
//! raw result set can therefore include code from other worktrees. Projection
//! decides what survives, per source:
//!
//! - **Code** in worktree-exact scope is kept only when its content hash is in
//!   this checkout's manifest, then mapped back to the local path. In a coarser
//!   scope (a repo or all-repos filter), the server filter already decided, so
//!   code passes through labeled by its stored path.
//! - **Slack / Linear** records have no checkout; the server-side metadata filter
//!   is authoritative, so they always pass through (an absent manifest hash is
//!   not a reason to drop them).
//! - **Web** results pass through only when the caller asked for them.

use mixedbread::Filter;
use search_meta::Source;

use crate::backend::{GrepOptions, SearchHit, SearchOptions, Store};
use crate::config::WEB_STORE;
use crate::error::Result;
use crate::manifest::Manifest;

/// A search result projected for display.
#[derive(Debug, Clone)]
pub struct DisplayHit {
    /// Repo-relative path, record title, or a URL for a web result.
    pub label: String,
    /// Which corpus the hit came from.
    pub source: Source,
    /// Zero-based start line within the file, when known.
    pub start_line: Option<u32>,
    /// Number of lines in the chunk (a count, not a span), when known.
    pub num_lines: Option<u32>,
    /// Relevance score in `0.0..=1.0`.
    pub score: f32,
    /// Matched snippet text.
    pub text: String,
}

/// A question-answering result projected for display.
#[derive(Debug, Clone)]
pub struct AnswerView {
    /// The synthesized answer.
    pub answer: String,
    /// Sources, filtered and mapped like search hits.
    pub sources: Vec<DisplayHit>,
}

/// How code hits are scoped. Record sources are always scoped by the server-side
/// metadata filter, so this only governs code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeScope {
    /// Keep only code whose content hash is in this checkout's manifest.
    WorktreeExact,
    /// Trust the server-side filter (a repo or all-repos query); do not intersect
    /// with the manifest.
    ServerFiltered,
}

/// Search `store_name` (and optionally the web store) and project the hits.
///
/// # Errors
/// Returns an error if the backend search request fails.
#[allow(clippy::too_many_arguments, reason = "thin pass-through of the query surface")]
pub async fn semantic(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    query: &str,
    top_k: usize,
    options: SearchOptions,
    include_web: bool,
    filters: Option<&Filter>,
    code_scope: CodeScope,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, include_web);
    let hits = store
        .search(&stores, query, overfetch(top_k), options, filters)
        .await?;
    Ok(project(manifest, hits, include_web, top_k, code_scope))
}

/// Grep `store_name` with a regular expression and project the hits.
///
/// # Errors
/// Returns an error if the pattern is not a valid regular expression or the
/// backend grep request fails.
#[allow(clippy::too_many_arguments, reason = "thin pass-through of the query surface")]
pub async fn grep(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    pattern: &str,
    top_k: usize,
    options: GrepOptions,
    filters: Option<&Filter>,
    code_scope: CodeScope,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, false);
    let hits = store
        .grep(&stores, pattern, overfetch(top_k), options, filters)
        .await?;
    Ok(project(manifest, hits, false, top_k, code_scope))
}

/// Ask a question against `store_name` (and optionally the web store).
///
/// # Errors
/// Returns an error if the backend request fails.
#[allow(clippy::too_many_arguments, reason = "thin pass-through of the query surface")]
pub async fn ask(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    query: &str,
    top_k: usize,
    options: SearchOptions,
    include_web: bool,
    filters: Option<&Filter>,
    code_scope: CodeScope,
) -> Result<AnswerView> {
    let stores = store_identifiers(store_name, include_web);
    let answer = store
        .ask(&stores, query, overfetch(top_k), options, filters)
        .await?;
    Ok(AnswerView {
        answer: answer.answer,
        sources: project(manifest, answer.sources, include_web, top_k, code_scope),
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
/// checkouts' code can crowd the raw top-k, so we ask for more than we show.
fn overfetch(top_k: usize) -> usize {
    top_k.saturating_mul(4).max(top_k.saturating_add(10))
}

fn project(
    manifest: &Manifest,
    hits: Vec<SearchHit>,
    include_web: bool,
    top_k: usize,
    code_scope: CodeScope,
) -> Vec<DisplayHit> {
    let local = manifest.hashes();
    let mut out = Vec::with_capacity(top_k);
    for hit in hits {
        if out.len() >= top_k {
            break;
        }
        match hit.source {
            Source::Web => {
                if include_web {
                    out.push(DisplayHit {
                        label: hit.path.unwrap_or_else(|| "(web)".to_owned()),
                        source: Source::Web,
                        start_line: None,
                        num_lines: None,
                        score: hit.score,
                        text: hit.text,
                    });
                }
            }
            Source::Code => {
                let in_manifest = hit.hash.as_deref().is_some_and(|hash| local.contains(hash));
                // Worktree-exact keeps only this checkout's code; a server-filtered
                // scope (a repo / all-repos query) trusts the backend filter.
                if code_scope == CodeScope::WorktreeExact && !in_manifest {
                    continue;
                }
                let label = hit
                    .hash
                    .as_deref()
                    .and_then(|hash| manifest.path_for_hash(hash))
                    .map(str::to_owned)
                    .or(hit.path)
                    .or(hit.hash)
                    .unwrap_or_default();
                out.push(DisplayHit {
                    label,
                    source: Source::Code,
                    start_line: hit.start_line,
                    num_lines: hit.num_lines,
                    score: hit.score,
                    text: hit.text,
                });
            }
            source @ (Source::Slack | Source::Linear) => {
                // No checkout to scope against; the server-side metadata filter is
                // authoritative, so the record passes through.
                out.push(DisplayHit {
                    label: hit.path.unwrap_or_default(),
                    source,
                    start_line: hit.start_line,
                    num_lines: hit.num_lines,
                    score: hit.score,
                    text: hit.text,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use search_meta::{Document, Source};

    use super::{CodeScope, grep, semantic};
    use crate::backend::{GrepOptions, GrepTargets, MemoryStore, SearchOptions, Store};
    use crate::content::ContentHash;
    use crate::manifest::{FileEntry, Manifest};

    fn opts() -> SearchOptions {
        SearchOptions {
            rerank: true,
            agentic: false,
        }
    }

    async fn put_code(store: &MemoryStore, path: &str, content: &str) -> ContentHash {
        let hash = ContentHash::of_bytes(content.as_bytes());
        let meta = serde_json::json!({
            "source": "code",
            "external_id": hash.as_str(),
            "content_hash": hash.as_str(),
            "title": path,
            "repo": "indexable-inc/index",
            "path": path,
        });
        store
            .upload(
                "s",
                Document {
                    external_id: hash.as_str().to_owned(),
                    file_name: path.to_owned(),
                    mime: "text/plain",
                    body: content.as_bytes().to_vec(),
                    meta_json: meta,
                    content_hash: hash.as_str().to_owned(),
                },
            )
            .await
            .expect("upload");
        hash
    }

    async fn put_slack(store: &MemoryStore, external_id: &str, title: &str, content: &str) {
        let hash = search_meta::hash_body(content.as_bytes());
        let meta = serde_json::json!({
            "source": "slack",
            "external_id": external_id,
            "content_hash": hash,
            "title": title,
            "channel_name": "craft",
        });
        store
            .upload(
                "s",
                Document {
                    external_id: external_id.to_owned(),
                    file_name: "thread.txt".to_owned(),
                    mime: "text/plain",
                    body: content.as_bytes().to_vec(),
                    meta_json: meta,
                    content_hash: hash,
                },
            )
            .await
            .expect("upload");
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
    async fn code_outside_this_checkout_is_filtered() {
        let store = MemoryStore::new();
        let mine = put_code(&store, "mine.rs", "needle in mine").await;
        let _theirs = put_code(&store, "theirs.rs", "needle in theirs").await;

        let manifest = manifest_with(&[("mine.rs", &mine)]);
        let hits = semantic(
            &store,
            "s",
            &manifest,
            "needle",
            10,
            opts(),
            false,
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "mine.rs");
        assert_eq!(hits[0].source, Source::Code);
    }

    #[tokio::test]
    async fn slack_passes_through_without_a_manifest_hash() {
        let store = MemoryStore::new();
        put_slack(&store, "slack:C0:1.2", "craft: ship it", "needle decision in slack").await;
        // The manifest knows no code; the Slack thread must still surface, because
        // record sources are scoped by the server filter, not the manifest.
        let manifest = Manifest::default();
        let hits = semantic(
            &store,
            "s",
            &manifest,
            "needle",
            10,
            opts(),
            false,
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, Source::Slack);
        assert_eq!(hits[0].label, "craft: ship it");
    }

    #[tokio::test]
    async fn source_filter_excludes_a_source() {
        let store = MemoryStore::new();
        let mine = put_code(&store, "mine.rs", "needle in code").await;
        put_slack(&store, "slack:C0:1.2", "craft", "needle in slack").await;
        let manifest = manifest_with(&[("mine.rs", &mine)]);

        // Exclude slack: only the code hit should survive.
        let filter = mixedbread::Filter::none(vec![mixedbread::Filter::eq("source", "slack")]);
        let hits = semantic(
            &store,
            "s",
            &manifest,
            "needle",
            10,
            opts(),
            false,
            Some(&filter),
            CodeScope::WorktreeExact,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, Source::Code);
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
        let alpha = put_code(&store, "alpha.rs", "fn handler() {}\nlet other = 1;").await;
        let beta = put_code(&store, "beta.rs", "struct Thing;\nfn render() {}").await;

        let manifest = manifest_with(&[("alpha.rs", &alpha), ("beta.rs", &beta)]);
        let hits = grep(
            &store,
            "s",
            &manifest,
            r"fn \w+\(\)",
            10,
            grep_opts(false),
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("grep");

        assert_eq!(hits.len(), 2);
        let mut labels: Vec<&str> = hits.iter().map(|hit| hit.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, vec!["alpha.rs", "beta.rs"]);
        assert!(hits.iter().all(|hit| hit.source == Source::Code));
    }

    #[tokio::test]
    async fn grep_case_sensitive_excludes_differently_cased_match() {
        let store = MemoryStore::new();
        let lower = put_code(&store, "lower.rs", "let token = read();").await;
        let upper = put_code(&store, "upper.rs", "let TOKEN = read();").await;

        let manifest = manifest_with(&[("lower.rs", &lower), ("upper.rs", &upper)]);

        let insensitive = grep(
            &store,
            "s",
            &manifest,
            "token",
            10,
            grep_opts(false),
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("grep");
        assert_eq!(insensitive.len(), 2);

        let sensitive = grep(
            &store,
            "s",
            &manifest,
            "token",
            10,
            grep_opts(true),
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("grep");
        assert_eq!(sensitive.len(), 1);
        assert_eq!(sensitive[0].label, "lower.rs");
    }

    #[tokio::test]
    async fn grep_invalid_pattern_is_an_error() {
        let store = MemoryStore::new();
        let only = put_code(&store, "only.rs", "anything").await;
        let manifest = manifest_with(&[("only.rs", &only)]);

        let result = grep(
            &store,
            "s",
            &manifest,
            "fn (",
            10,
            grep_opts(false),
            None,
            CodeScope::WorktreeExact,
        )
        .await;
        assert!(result.is_err());
    }
}
