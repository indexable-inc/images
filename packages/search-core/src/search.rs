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

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use mixedbread::{Filter, SortBy};
use regex::Regex;
use source_meta::{KNOWN_SOURCE_TAGS, Source, keys};

use crate::backend::{AskOptions, GrepOptions, SearchHit, SearchOptions, Store};
use crate::config::WEB_STORE;
use crate::error::Result;
use crate::manifest::Manifest;

/// Snippet cap applied by [`RenderMode::Compact`], in characters.
///
/// Chosen so a default `top_k = 10` response stays in the low thousands of
/// tokens (the uncapped median chunk measured ~8 KiB, ~2k tokens, per hit).
pub const COMPACT_SNIPPET_CHARS: usize = 400;

/// How hits are projected for consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// Every chunk passes through with its full text.
    #[default]
    Full,
    /// Token-frugal projection for agents: overlapping chunks of the same
    /// document are collapsed to the best-scoring one (refilled from the
    /// overfetch buffer, so `top_k` distinct documents still come back) and
    /// each snippet is capped at [`COMPACT_SNIPPET_CHARS`] characters.
    Compact,
}

/// A search result projected for display.
///
/// Serializes to the stable `search --json` object. `label` is renamed to `path`
/// there to match the [`search-py`](../search-py) binding's dict, the other
/// established machine-readable contract over the same hits. The provenance
/// fields (`timestamp` through `project`) are skipped when absent, so sources
/// that never write them add no key.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DisplayHit {
    /// Repo-relative path, record title, or a URL for a web result.
    #[serde(rename = "path")]
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
    /// Epoch-second timestamp of the record (the primary recency axis).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// OS user that authored the record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Short hostname the record was recorded on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Session id (Claude Code transcript, codex, or shell session).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The record's caller-assigned external id (e.g. `claude:{session}:{uuid}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// Canonical web URL (GitHub items, Linear issues).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Repository slug for code and git-commit records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Project slug (the working directory a transcript was recorded under).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl DisplayHit {
    /// Build a hit from a backend [`SearchHit`] under `label`, carrying the
    /// provenance through. Construction lives here so every projection branch
    /// keeps the same metadata.
    // `pub(crate)`: the context view (`context.rs`) projects its turns through
    // the same constructor; the lint fires because the module is private, but
    // the method is shared via the crate, not the public API.
    #[allow(clippy::redundant_pub_crate)]
    pub(crate) fn from_hit(label: String, hit: SearchHit) -> Self {
        let provenance = hit.provenance;
        Self {
            label,
            source: hit.source,
            start_line: hit.start_line,
            num_lines: hit.num_lines,
            score: hit.score,
            text: hit.text,
            timestamp: provenance.timestamp,
            user: provenance.user,
            host: provenance.host,
            session_id: provenance.session_id,
            external_id: provenance.external_id,
            url: provenance.url,
            repo: provenance.repo,
            project: provenance.project,
        }
    }
}

/// Serialize hits as the machine-readable JSON array `search --json` prints.
///
/// Lives here, at the owner of [`DisplayHit`], so both the CLI and any other
/// consumer share one serialization and the JSON shape stays defined in one
/// place.
///
/// # Errors
/// Returns an error if serialization fails, which is not expected for these
/// scalar/string fields.
pub fn hits_to_json(hits: &[DisplayHit]) -> serde_json::Result<String> {
    serde_json::to_string(hits)
}

/// A question-answering result projected for display.
#[derive(Debug, Clone)]
pub struct AnswerView {
    /// The synthesized answer. The backend's raw `<cite i="N"/>` markers
    /// (indices into its own over-fetched source list) are rewritten to `[n]`,
    /// where `n` is a 0-based index into `sources`; a marker whose source the
    /// projection excludes (or that cites no real source) is dropped, so every
    /// surviving citation resolves against `sources`.
    pub answer: String,
    /// Sources, filtered and mapped like search hits. Cited sources are always
    /// present, even past the `top_k` display cap.
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
#[allow(
    clippy::too_many_arguments,
    reason = "thin pass-through of the query surface"
)]
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
    mode: RenderMode,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, include_web);
    let hits = store
        .search(&stores, query, overfetch(top_k), options, filters)
        .await?;
    Ok(project(manifest, hits, include_web, top_k, code_scope, mode))
}

/// Grep `store_name` with a regular expression and project the hits.
///
/// # Errors
/// Returns an error if the pattern is not a valid regular expression or the
/// backend grep request fails.
#[allow(
    clippy::too_many_arguments,
    reason = "thin pass-through of the query surface"
)]
pub async fn grep(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    pattern: &str,
    top_k: usize,
    options: GrepOptions,
    filters: Option<&Filter>,
    code_scope: CodeScope,
    mode: RenderMode,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, false);
    let hits = store
        .grep(&stores, pattern, overfetch(top_k), options, filters)
        .await?;
    Ok(project(manifest, hits, false, top_k, code_scope, mode))
}

/// List the newest records (descending [`keys::TIMESTAMP`]) matching `filters`.
///
/// A deterministic recency feed with no semantic scoring, backed by the
/// store's metadata-only chunk listing. The score on each hit is the API's
/// placeholder, not a relevance signal.
///
/// # Errors
/// Returns an error if the backend listing fails.
pub async fn recent(
    store: &(impl Store + Sync),
    store_name: &str,
    top_k: usize,
    filters: Option<&Filter>,
    mode: RenderMode,
) -> Result<Vec<DisplayHit>> {
    ranked(
        store,
        store_name,
        top_k,
        filters,
        &SortBy::desc(keys::TIMESTAMP),
        mode,
    )
    .await
}

/// List records matching `filters` ranked by an arbitrary metadata `sort`.
///
/// The general form of [`recent`], for when the ranking axis comes from the
/// caller (e.g. a query-enhance `sort` item). No semantic scoring happens, so
/// scores are the API's placeholder.
///
/// # Errors
/// Returns an error if the backend listing fails.
pub async fn ranked(
    store: &(impl Store + Sync),
    store_name: &str,
    top_k: usize,
    filters: Option<&Filter>,
    sort: &SortBy,
    mode: RenderMode,
) -> Result<Vec<DisplayHit>> {
    let stores = store_identifiers(store_name, false);
    // Overfetch only matters in compact mode, where duplicate documents are
    // collapsed and the feed refills from the buffer.
    let fetch = match mode {
        RenderMode::Full => top_k,
        RenderMode::Compact => overfetch(top_k),
    };
    let hits = store.list_chunks(&stores, fetch, filters, Some(sort)).await?;
    // No manifest: this is a pure server-side listing, so code hits pass
    // through labeled by their stored path.
    Ok(project(
        &Manifest::default(),
        hits,
        false,
        top_k,
        CodeScope::ServerFiltered,
        mode,
    ))
}

/// One source's census row: how many documents the store holds for it and
/// the newest record's timestamp. Serializes to the `search stats --json`
/// object.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceStat {
    /// The source tag (e.g. `shell`, `claude_history`).
    pub source: String,
    /// Documents (store files) the store holds for this source.
    pub documents: u64,
    /// True when `documents` hit the backend's bounded facet-scan cap
    /// ([`mixedbread::FACETS_MAX_FILES`]) and is therefore a lower bound.
    /// Skipped from JSON when exact, so the common row stays small.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Epoch-second timestamp of the source's newest record, when any record
    /// carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_timestamp: Option<i64>,
}

/// Census the corpus: per-source document counts and freshness (the newest
/// [`keys::TIMESTAMP`] per source).
///
/// Rows come back sorted by count descending, then source name, so the
/// dominant sources lead; sources with no matching document yield no row.
///
/// One discovery facet call finds which source tags exist; that scan is
/// bounded ([`mixedbread::FACETS_MAX_FILES`] files), so the candidates are
/// unioned with the canonical tags in case a small source hides entirely
/// behind a dominant one. Each candidate is then counted by its own
/// source-scoped facet call — the scan bound applies per source instead of
/// being shared across the whole store — and probed for freshness with a
/// single-chunk ranked listing, all concurrently. A source bigger than the
/// scan bound still reports the cap, marked [`SourceStat::truncated`]
/// (verified live: `claude_history` alone exceeds it, 2026-06-12).
///
/// `filters` narrows the census the same way it narrows a search (host, user,
/// time window, ...); counts and freshness then describe the scoped slice.
///
/// # Errors
/// Returns an error if any facet call or per-source listing fails.
pub async fn stats(
    store: &(impl Store + Sync),
    store_name: &str,
    filters: Option<&Filter>,
) -> Result<Vec<SourceStat>> {
    let stores = store_identifiers(store_name, false);
    let mut discovered = store.facets(&stores, &[keys::SOURCE], filters).await?;
    let candidates: std::collections::BTreeSet<String> = discovered
        .remove(keys::SOURCE)
        .unwrap_or_default()
        .into_keys()
        .chain(KNOWN_SOURCE_TAGS.iter().map(ToString::to_string))
        .collect();

    let rows = futures::future::try_join_all(candidates.into_iter().map(|source| {
        let stores = &stores;
        async move {
            let scoped = Filter::eq(keys::SOURCE, source.as_str());
            let merged = match filters {
                Some(filter) => Filter::all(vec![filter.clone(), scoped]),
                None => scoped,
            };

            let counts = store.facets(stores, &[keys::SOURCE], Some(&merged)).await?;
            let documents = counts
                .get(keys::SOURCE)
                .and_then(|values| values.get(&source))
                .copied()
                .unwrap_or(0);
            if documents == 0 {
                return Ok::<Option<SourceStat>, crate::error::Error>(None);
            }

            // The newest record of the source: a metadata-only listing of its
            // single top chunk by descending timestamp, under the same scope.
            let newest = store
                .list_chunks(stores, 1, Some(&merged), Some(&SortBy::desc(keys::TIMESTAMP)))
                .await?
                .first()
                .and_then(|hit| hit.provenance.timestamp);
            Ok(Some(SourceStat {
                source,
                documents,
                truncated: documents >= u64::from(mixedbread::FACETS_MAX_FILES),
                newest_timestamp: newest,
            }))
        }
    }))
    .await?;

    let mut rows: Vec<SourceStat> = rows.into_iter().flatten().collect();
    rows.sort_by(|a, b| {
        b.documents
            .cmp(&a.documents)
            .then_with(|| a.source.cmp(&b.source))
    });
    Ok(rows)
}

/// Ask a question against `store_name` (and optionally the web store).
///
/// The answer's citation markers are remapped from the backend's raw source
/// list onto the projected `sources` (see [`AnswerView::answer`]), so a
/// consumer can always resolve `[n]` against the returned list.
///
/// # Errors
/// Returns an error if the backend request fails.
#[allow(
    clippy::too_many_arguments,
    reason = "thin pass-through of the query surface"
)]
pub async fn ask(
    store: &(impl Store + Sync),
    store_name: &str,
    manifest: &Manifest,
    query: &str,
    top_k: usize,
    options: AskOptions,
    include_web: bool,
    filters: Option<&Filter>,
    code_scope: CodeScope,
) -> Result<AnswerView> {
    let stores = store_identifiers(store_name, include_web);
    let answer = store
        .ask(&stores, query, overfetch(top_k), options, filters)
        .await?;
    // Project each raw source individually, preserving its raw position: the
    // answer's citation markers index the backend's over-fetched list, so the
    // projection must remember where every display came from before the list
    // is filtered and capped, or the indices dangle.
    let local = manifest.hashes();
    let displays: Vec<Option<DisplayHit>> = answer
        .sources
        .into_iter()
        .map(|hit| display_hit(manifest, &local, hit, include_web, code_scope))
        .collect();
    Ok(align_citations(&answer.answer, displays, top_k))
}

/// The backend's citation marker: `<cite i="N"/>`, `N` a 0-based index into
/// the raw source list the question-answering call returned.
static CITE_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<cite\s+i="(\d+)"\s*/>"#).expect("static citation pattern compiles")
});

/// Rewrite an answer's citation markers against the projected source list.
///
/// `displays` is the per-raw-index projection of the backend's sources
/// (`None` where the projection's source rules exclude a hit). The first
/// `top_k` surviving hits form the displayed list, mirroring [`project`]'s
/// cap; a citation of a hit past the cap appends that hit so the citation
/// still resolves, and a citation of an excluded or nonexistent source is
/// dropped from the text.
fn align_citations(
    answer: &str,
    mut displays: Vec<Option<DisplayHit>>,
    top_k: usize,
) -> AnswerView {
    use std::fmt::Write as _;

    // The displayed list starts as the first `top_k` projected hits, each
    // remembering the raw index it came from.
    let mut sources: Vec<DisplayHit> = Vec::new();
    let mut position: HashMap<usize, usize> = HashMap::new();
    for (raw, slot) in displays.iter_mut().enumerate() {
        if sources.len() >= top_k {
            break;
        }
        if let Some(display) = slot.take() {
            position.insert(raw, sources.len());
            sources.push(display);
        }
    }

    let mut rewritten = String::with_capacity(answer.len());
    let mut tail = 0;
    for captures in CITE_MARKER.captures_iter(answer) {
        let marker = captures.get(0).expect("regex match has a whole match");
        rewritten.push_str(&answer[tail..marker.start()]);
        tail = marker.end();
        // A non-parsing index (absurdly large digits) cites nothing real, so
        // it is dropped like an out-of-range one.
        let Ok(raw) = captures[1].parse::<usize>() else {
            continue;
        };
        let projected = position.get(&raw).copied().or_else(|| {
            // Cited but not displayed: a hit past the `top_k` cap. Append it so
            // the citation resolves; an excluded (`None`) or out-of-range index
            // stays unresolved and the marker is dropped.
            let display = displays.get_mut(raw).and_then(Option::take)?;
            position.insert(raw, sources.len());
            sources.push(display);
            Some(sources.len() - 1)
        });
        if let Some(index) = projected {
            write!(rewritten, "[{index}]").expect("writing to a String cannot fail");
        }
    }
    rewritten.push_str(&answer[tail..]);

    AnswerView {
        answer: rewritten,
        sources,
    }
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
// `pub(crate)`: the context view (`context.rs`) over-fetches its chunk
// listings under the same policy; the lint fires because the module is
// private, but the function is shared via the crate, not the public API.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn overfetch(top_k: usize) -> usize {
    top_k.saturating_mul(4).max(top_k.saturating_add(10))
}

/// Project one backend hit into its display form, or `None` when the
/// projection's source rules exclude it: a web hit the caller did not ask
/// for, or worktree-exact code whose hash is not in this checkout's manifest.
///
/// `local` is the manifest's hash set, computed once by the caller so a long
/// hit list does not rebuild it per hit.
fn display_hit(
    manifest: &Manifest,
    local: &HashSet<&str>,
    hit: SearchHit,
    include_web: bool,
    code_scope: CodeScope,
) -> Option<DisplayHit> {
    let source = hit.source.clone();
    if source.is_web() {
        if !include_web {
            return None;
        }
        let label = hit.path.clone().unwrap_or_else(|| "(web)".to_owned());
        let mut display = DisplayHit::from_hit(label, hit);
        // Web chunks report line metadata that is meaningless for a page;
        // keep the established shape of web hits line-free.
        display.start_line = None;
        display.num_lines = None;
        Some(display)
    } else if source.is_code() {
        let in_manifest = hit.hash.as_deref().is_some_and(|hash| local.contains(hash));
        // Worktree-exact keeps only this checkout's code; a server-filtered
        // scope (a repo / all-repos query) trusts the backend filter.
        if code_scope == CodeScope::WorktreeExact && !in_manifest {
            return None;
        }
        let label = hit
            .hash
            .as_deref()
            .and_then(|hash| manifest.path_for_hash(hash))
            .map(str::to_owned)
            .or_else(|| hit.path.clone())
            .or_else(|| hit.hash.clone())
            .unwrap_or_default();
        Some(DisplayHit::from_hit(label, hit))
    } else {
        // Any other tag (slack, linear, claude_history, ...) is a record
        // source: no checkout to scope against, so the server-side metadata
        // filter is authoritative and the record passes through.
        let label = hit.path.clone().unwrap_or_default();
        Some(DisplayHit::from_hit(label, hit))
    }
}

fn project(
    manifest: &Manifest,
    hits: Vec<SearchHit>,
    include_web: bool,
    top_k: usize,
    code_scope: CodeScope,
    mode: RenderMode,
) -> Vec<DisplayHit> {
    // Compact mode collapses repeated chunks of one document: the raw top-k is
    // often dominated by overlapping chunks of a single file. Hits arrive
    // relevance-sorted (the reranker's order), so the first chunk seen for a
    // document is its best-scoring one; later chunks are dropped and the list
    // refills from the overfetch buffer.
    let local = manifest.hashes();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(top_k);
    for hit in hits {
        if out.len() >= top_k {
            break;
        }
        let Some(display) = display_hit(manifest, &local, hit, include_web, code_scope) else {
            continue;
        };
        if mode == RenderMode::Compact {
            if !seen.insert(document_key(&display)) {
                continue;
            }
            out.push(truncate_snippet(display, COMPACT_SNIPPET_CHARS));
        } else {
            out.push(display);
        }
    }
    out
}

/// The identity under which compact mode collapses chunks: the stored
/// `external_id` when present (one per document), else source + label, which
/// groups chunks of one file or record.
fn document_key(hit: &DisplayHit) -> String {
    hit.external_id.clone().unwrap_or_else(|| {
        format!("{}\u{1f}{}", hit.source.as_str(), hit.label)
    })
}

/// Cap a hit's snippet at `max_chars` characters (on a char boundary), marking
/// the cut with an ellipsis. `start_line`/`num_lines` keep describing the full
/// chunk, so the pointer back into the source stays valid.
fn truncate_snippet(mut hit: DisplayHit, max_chars: usize) -> DisplayHit {
    if hit.text.chars().count() > max_chars {
        let mut text: String = hit.text.chars().take(max_chars).collect();
        text.push('…');
        hit.text = text;
    }
    hit
}

#[cfg(test)]
mod tests {
    use source_meta::{Document, Source};

    use super::{
        CodeScope, DisplayHit, RenderMode, align_citations, ask, grep, recent, semantic, stats,
    };
    use crate::backend::{GrepOptions, GrepTargets, MemoryStore, SearchOptions, Store};
    use crate::content::ContentHash;
    use crate::manifest::{FileEntry, Manifest};

    fn opts() -> SearchOptions {
        SearchOptions::default()
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
        let hash = source_meta::hash_body(content.as_bytes());
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
            RenderMode::Full,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "mine.rs");
        assert_eq!(hits[0].source, Source::code());
    }

    #[tokio::test]
    async fn slack_passes_through_without_a_manifest_hash() {
        let store = MemoryStore::new();
        put_slack(
            &store,
            "slack:C0:1.2",
            "craft: ship it",
            "needle decision in slack",
        )
        .await;
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
            RenderMode::Full,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, Source::new("slack"));
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
            RenderMode::Full,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, Source::code());
    }

    /// Upload a claude_history-shaped record with full provenance metadata.
    async fn put_history(
        store: &MemoryStore,
        external_id: &str,
        title: &str,
        content: &str,
        timestamp: i64,
    ) {
        let hash = source_meta::hash_body(content.as_bytes());
        let meta = serde_json::json!({
            "source": "claude_history",
            "external_id": external_id,
            "content_hash": hash,
            "title": title,
            "timestamp": timestamp,
            "user": "andrew",
            "host": "hydra",
            "session_id": "sess-1",
            "project": "/home/andrew/index",
        });
        store
            .upload(
                "s",
                Document {
                    external_id: external_id.to_owned(),
                    file_name: "message.txt".to_owned(),
                    mime: "text/plain",
                    body: content.as_bytes().to_vec(),
                    meta_json: meta,
                    content_hash: hash,
                },
            )
            .await
            .expect("upload");
    }

    #[tokio::test]
    async fn hits_carry_provenance_metadata() {
        let store = MemoryStore::new();
        put_history(
            &store,
            "claude:sess-1:uuid-1",
            "assistant @ index: fixed the bug",
            "needle: the fix was reverting the cursor",
            1_781_000_000,
        )
        .await;

        let hits = semantic(
            &store,
            "s",
            &Manifest::default(),
            "needle",
            10,
            opts(),
            false,
            None,
            CodeScope::WorktreeExact,
            RenderMode::Full,
        )
        .await
        .expect("search");

        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.timestamp, Some(1_781_000_000));
        assert_eq!(hit.user.as_deref(), Some("andrew"));
        assert_eq!(hit.host.as_deref(), Some("hydra"));
        assert_eq!(hit.session_id.as_deref(), Some("sess-1"));
        assert_eq!(hit.external_id.as_deref(), Some("claude:sess-1:uuid-1"));
        assert_eq!(hit.project.as_deref(), Some("/home/andrew/index"));

        // The JSON contract: provenance keys present, and no `url`/`repo` keys
        // for a record that never wrote them.
        let json: serde_json::Value =
            serde_json::from_str(&super::hits_to_json(&hits).expect("json")).expect("parse");
        assert_eq!(json[0]["session_id"], "sess-1");
        assert_eq!(json[0]["timestamp"], 1_781_000_000);
        assert!(json[0].get("url").is_none(), "absent url adds no key");
        assert!(json[0].get("repo").is_none(), "absent repo adds no key");
    }

    #[tokio::test]
    async fn compact_collapses_chunks_of_one_document_and_caps_snippets() {
        let store = MemoryStore::new();
        // One document whose body matches the query on many lines: the
        // MemoryStore emits one chunk per matching line, modeling the
        // overlapping-chunks-of-one-file pathology.
        let long_line = format!("needle {}", "x".repeat(600));
        let body = format!("{long_line}\nneedle two\nneedle three");
        put_history(&store, "claude:sess-1:uuid-1", "spam", &body, 1).await;
        put_history(&store, "claude:sess-1:uuid-2", "other", "needle other", 2).await;

        let compact = semantic(
            &store,
            "s",
            &Manifest::default(),
            "needle",
            10,
            opts(),
            false,
            None,
            CodeScope::WorktreeExact,
            RenderMode::Compact,
        )
        .await
        .expect("search");
        // Two documents, not four chunks.
        assert_eq!(compact.len(), 2);
        let mut ids: Vec<&str> = compact
            .iter()
            .filter_map(|hit| hit.external_id.as_deref())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["claude:sess-1:uuid-1", "claude:sess-1:uuid-2"]);
        // The long snippet is capped with an ellipsis marker.
        let long_hit = compact
            .iter()
            .find(|hit| hit.external_id.as_deref() == Some("claude:sess-1:uuid-1"))
            .expect("long hit");
        assert!(long_hit.text.chars().count() <= super::COMPACT_SNIPPET_CHARS + 1);
        assert!(long_hit.text.ends_with('…'));

        // Full mode keeps every chunk and the whole text.
        let full = semantic(
            &store,
            "s",
            &Manifest::default(),
            "needle",
            10,
            opts(),
            false,
            None,
            CodeScope::WorktreeExact,
            RenderMode::Full,
        )
        .await
        .expect("search");
        assert_eq!(full.len(), 4);
        assert!(full.iter().any(|hit| hit.text.len() > 600));
    }

    #[tokio::test]
    async fn recent_lists_newest_first_and_honors_since() {
        let store = MemoryStore::new();
        put_history(&store, "id:old", "old", "alpha", 1_000).await;
        put_history(&store, "id:mid", "mid", "beta", 2_000).await;
        put_history(&store, "id:new", "new", "gamma", 3_000).await;

        let hits = recent(&store, "s", 10, None, RenderMode::Full)
            .await
            .expect("recent");
        let stamps: Vec<i64> = hits.iter().filter_map(|hit| hit.timestamp).collect();
        assert_eq!(stamps, vec![3_000, 2_000, 1_000], "newest first");

        // A since window built through the shared FilterSpec excludes the old
        // record server-side (the MemoryStore models numeric gte).
        let spec = crate::FilterSpec {
            since: Some(1_500),
            ..crate::FilterSpec::default()
        };
        let filter = crate::build_filter(&spec).expect("filter");
        let windowed = recent(&store, "s", 10, Some(&filter), RenderMode::Full)
            .await
            .expect("recent windowed");
        let stamps: Vec<i64> = windowed.iter().filter_map(|hit| hit.timestamp).collect();
        assert_eq!(stamps, vec![3_000, 2_000]);
    }

    #[tokio::test]
    async fn stats_counts_documents_and_reports_freshness_per_source() {
        let store = MemoryStore::new();
        put_history(&store, "id:a", "a", "alpha", 1_000).await;
        put_history(&store, "id:b", "b", "beta", 3_000).await;
        put_slack(&store, "slack:C0:1.2", "craft", "gamma").await;

        let rows = stats(&store, "s", None).await.expect("stats");

        // Dominant source first; counts are per document, and freshness is
        // each source's newest timestamp (the slack fixture writes none).
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "claude_history");
        assert_eq!(rows[0].documents, 2);
        assert_eq!(rows[0].newest_timestamp, Some(3_000));
        assert_eq!(rows[1].source, "slack");
        assert_eq!(rows[1].documents, 1);
        assert_eq!(rows[1].newest_timestamp, None);

        // A scope filter narrows both the counts and the freshness probe: a
        // window that excludes the newer record reports the older one.
        let spec = crate::FilterSpec {
            until: Some(2_000),
            ..crate::FilterSpec::default()
        };
        let filter = crate::build_filter(&spec).expect("filter");
        let windowed = stats(&store, "s", Some(&filter)).await.expect("stats");
        assert_eq!(windowed.len(), 1, "the timestamp-free slack row drops out");
        assert_eq!(windowed[0].source, "claude_history");
        assert_eq!(windowed[0].documents, 1);
        assert_eq!(windowed[0].newest_timestamp, Some(1_000));
    }

    #[tokio::test]
    async fn answer_citations_reference_the_projected_list() {
        let store = MemoryStore::new();
        put_history(&store, "id:a", "a", "needle alpha", 1).await;
        put_history(&store, "id:b", "b", "needle beta", 2).await;
        put_history(&store, "id:c", "c", "needle gamma", 3).await;

        // The regression: `top_k = 1` caps the displayed list below the raw
        // source count, while the backend's answer cites every raw source.
        // Without remapping, the second and third markers would index past
        // (or at the wrong entry of) the one projected hit.
        let view = ask(
            &store,
            "s",
            &Manifest::default(),
            "needle",
            1,
            opts().into(),
            false,
            None,
            CodeScope::WorktreeExact,
        )
        .await
        .expect("ask");

        // Raw markers are gone, rewritten onto the projected list in citation
        // order: the kept hit first, then the cited-but-truncated ones appended.
        assert!(!view.answer.contains("<cite"), "{}", view.answer);
        assert!(view.answer.ends_with("[0][1][2]"), "{}", view.answer);
        assert_eq!(view.sources.len(), 3, "cited sources survive the cap");
        let mut ids: Vec<&str> = view
            .sources
            .iter()
            .filter_map(|hit| hit.external_id.as_deref())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["id:a", "id:b", "id:c"]);
    }

    /// A minimal display hit for exercising [`align_citations`] directly.
    fn cited(label: &str) -> DisplayHit {
        DisplayHit {
            label: label.to_owned(),
            source: Source::new("claude_history"),
            start_line: None,
            num_lines: None,
            score: 0.5,
            text: String::new(),
            timestamp: None,
            user: None,
            host: None,
            session_id: None,
            external_id: None,
            url: None,
            repo: None,
            project: None,
        }
    }

    #[test]
    fn dangling_citations_are_dropped_and_surviving_ones_renumbered() {
        // Raw sources: index 1 was excluded by the projection (`None`), index 3
        // is past the `top_k = 2` display cap, and index 9 does not exist.
        let displays = vec![Some(cited("a")), None, Some(cited("b")), Some(cited("c"))];
        let answer = r#"x <cite i="0"/> y <cite i="3"/> z <cite i="1"/> w <cite i="9"/>."#;

        let view = align_citations(answer, displays, 2);

        // 0 keeps its slot, 3 is appended past the cap and renumbered, the
        // excluded and nonexistent citations are dropped from the text.
        assert_eq!(view.answer, "x [0] y [2] z  w .");
        let labels: Vec<&str> = view.sources.iter().map(|hit| hit.label.as_str()).collect();
        assert_eq!(labels, vec!["a", "b", "c"]);
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
            RenderMode::Full,
        )
        .await
        .expect("grep");

        assert_eq!(hits.len(), 2);
        let mut labels: Vec<&str> = hits.iter().map(|hit| hit.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, vec!["alpha.rs", "beta.rs"]);
        assert!(hits.iter().all(|hit| hit.source.is_code()));
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
            RenderMode::Full,
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
            RenderMode::Full,
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
            RenderMode::Full,
        )
        .await;
        assert!(result.is_err());
    }
}
