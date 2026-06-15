//! Expanding one corpus record into its surrounding conversation.
//!
//! A search hit from a transcript source (`claude_history`, `codex`, `shell`)
//! is one turn of a longer session. Given that hit's `external_id`, this
//! module fetches the turns around it — the same `session_id`, ordered by
//! timestamp — via the store's metadata-only chunk listing, so a consumer can
//! read the conversation a hit came from instead of a lone snippet. Sources
//! that carry no session (`git`, `github`, `code`) fall back to the record's
//! own chunks in document order, and a bare session id lists that session
//! from its start.

use std::collections::HashSet;

use mixedbread::{Filter, Operator, SortBy};
use snafu::ensure;
use source_meta::keys;

use crate::backend::{SearchHit, Store};
use crate::error::{ContextNotFoundSnafu, Result};
use crate::search::{DisplayHit, overfetch};

/// Chunk-order sort field: chunks of one record are returned in the order of
/// their start line within the record body.
const CHUNK_ORDER: &str = "generated_metadata.start_line";

/// A record expanded into its surrounding conversation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextView {
    /// Conversation turns in ascending timestamp order.
    pub turns: Vec<DisplayHit>,
    /// Index into `turns` of the requested record, when the view is a session
    /// window around it. `None` when every turn already belongs to the
    /// requested id: the same-record fallback for sources without a session,
    /// or a session id passed directly (the view starts at the session head).
    pub anchor: Option<usize>,
}

/// Fetch the conversation surrounding the record `id` (a hit's `external_id`,
/// or a bare session id): up to `before` turns of the same session before it
/// and `after` turns after it, ordered by timestamp.
///
/// One record may span several stored chunks; the view keeps one turn per
/// record (its first listed chunk), except in the same-record fallback, where
/// the record's own chunks are the turns, in document order.
///
/// # Errors
/// Returns an error if a backend listing fails, or
/// [`Error::ContextNotFound`](crate::Error::ContextNotFound) when the id
/// matches no record and no session.
pub async fn context(
    store: &(impl Store + Sync),
    store_name: &str,
    id: &str,
    before: usize,
    after: usize,
) -> Result<ContextView> {
    let stores = vec![store_name.to_owned()];
    let span = before.saturating_add(after).saturating_add(1);

    // The id is normally a hit's `external_id`; its own chunks resolve the
    // record's provenance (session, timestamp) and double as the fallback
    // view for sources without a session.
    let by_line = SortBy::asc(CHUNK_ORDER);
    let own_chunks = store
        .list_chunks(
            &stores,
            span,
            Some(&Filter::eq(keys::EXTERNAL_ID, id)),
            Some(&by_line),
        )
        .await?;

    let Some(anchor) = own_chunks.first().cloned() else {
        // Nothing stored under that external id: treat the id as a session id
        // and list the session from its start.
        return session_from_start(store, &stores, id, span).await;
    };

    if let (Some(session), Some(timestamp)) =
        (anchor.provenance.session_id.clone(), anchor.provenance.timestamp)
    {
        return window_around(
            store, &stores, anchor, id, &session, timestamp, before, after,
        )
        .await;
    }

    // No session (git, github, code) or no timestamp axis to order by: the
    // record's own chunks, in document order, are the whole context.
    Ok(ContextView {
        turns: own_chunks.into_iter().map(turn).collect(),
        anchor: None,
    })
}

/// The session window around the anchor record: `before` earlier turns, the
/// anchor, then `after` later turns, all ascending by timestamp.
#[allow(
    clippy::too_many_arguments,
    reason = "internal continuation of `context`, splitting the deep branch"
)]
async fn window_around(
    store: &(impl Store + Sync),
    stores: &[String],
    anchor: SearchHit,
    id: &str,
    session: &str,
    timestamp: i64,
    before: usize,
    after: usize,
) -> Result<ContextView> {
    // Scope both directions to the anchor's session AND source: a transcript
    // session id is shared with its debug log (`claude_history` /
    // `claude_debug`), and mixing the two would interleave internal records
    // into the conversation.
    let scope = |operator: Operator| {
        Filter::all(vec![
            Filter::eq(keys::SESSION_ID, session),
            Filter::eq(keys::SOURCE, anchor.source.as_str()),
            Filter::condition(keys::TIMESTAMP, operator, timestamp),
        ])
    };
    // Over-fetch both directions: long records contribute several chunks per
    // turn, so `n` turns can need well over `n` chunks.
    let earlier = store
        .list_chunks(
            stores,
            overfetch(before.saturating_add(1)),
            Some(&scope(Operator::Lte)),
            Some(&SortBy::desc(keys::TIMESTAMP)),
        )
        .await?;
    let later = store
        .list_chunks(
            stores,
            overfetch(after.saturating_add(1)),
            Some(&scope(Operator::Gte)),
            Some(&SortBy::asc(keys::TIMESTAMP)),
        )
        .await?;

    // One turn per record. Records sharing the anchor's exact timestamp can
    // appear in both directions; the `taken` set keeps each on one side.
    let mut taken: HashSet<String> = HashSet::new();
    taken.insert(id.to_owned());

    let mut earlier = dedupe_turns(earlier);
    earlier.retain(|turn| {
        turn.provenance
            .external_id
            .as_ref()
            .is_none_or(|eid| taken.insert(eid.clone()))
    });
    earlier.truncate(before);
    // The earlier window was fetched newest-first to land next to the anchor;
    // the conversation reads oldest-first.
    earlier.reverse();

    let anchor_index = earlier.len();
    let mut turns = earlier;
    turns.push(anchor);
    for candidate in dedupe_turns(later) {
        if turns.len() > anchor_index.saturating_add(after) {
            break;
        }
        if candidate
            .provenance
            .external_id
            .as_ref()
            .is_none_or(|eid| taken.insert(eid.clone()))
        {
            turns.push(candidate);
        }
    }

    Ok(ContextView {
        turns: turns.into_iter().map(turn).collect(),
        anchor: Some(anchor_index),
    })
}

/// The fallback for an id that names no record: list it as a session from its
/// start (ascending timestamp), capped at `span` turns.
async fn session_from_start(
    store: &(impl Store + Sync),
    stores: &[String],
    id: &str,
    span: usize,
) -> Result<ContextView> {
    let chunks = store
        .list_chunks(
            stores,
            overfetch(span),
            Some(&Filter::eq(keys::SESSION_ID, id)),
            Some(&SortBy::asc(keys::TIMESTAMP)),
        )
        .await?;
    ensure!(!chunks.is_empty(), ContextNotFoundSnafu { id });

    let mut turns = dedupe_turns(chunks);
    turns.truncate(span);
    Ok(ContextView {
        turns: turns.into_iter().map(turn).collect(),
        anchor: None,
    })
}

/// Collapse a chunk listing to one chunk per record (the first listed),
/// preserving order. Chunks without an `external_id` cannot be grouped and
/// pass through as their own turns.
fn dedupe_turns(chunks: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut seen: HashSet<String> = HashSet::new();
    chunks
        .into_iter()
        .filter(|chunk| {
            chunk
                .provenance
                .external_id
                .as_ref()
                .is_none_or(|eid| seen.insert(eid.clone()))
        })
        .collect()
}

/// Project one turn for display. Context turns come from a metadata listing
/// the server already scoped, so nothing is filtered: records label by title,
/// code by stored path (with the content hash as the last resort).
fn turn(hit: SearchHit) -> DisplayHit {
    let label = hit
        .path
        .clone()
        .or_else(|| hit.hash.clone())
        .unwrap_or_default();
    DisplayHit::from_hit(label, hit)
}

#[cfg(test)]
mod tests {
    use source_meta::Document;

    use super::context;
    use crate::backend::{MemoryStore, Store as _};

    /// Upload one transcript turn: a `claude_history` record with a session
    /// and a timestamp.
    async fn put_turn(store: &MemoryStore, session: &str, uuid: &str, body: &str, timestamp: i64) {
        let external_id = format!("claude:{session}:{uuid}");
        let hash = source_meta::hash_body(body.as_bytes());
        let meta = serde_json::json!({
            "source": "claude_history",
            "external_id": external_id,
            "content_hash": hash,
            "title": format!("user @ proj: {body}"),
            "timestamp": timestamp,
            "session_id": session,
            "user": "andrew",
            "host": "hydra",
        });
        store
            .upload(
                "s",
                Document {
                    external_id,
                    file_name: "message.txt".to_owned(),
                    mime: "text/plain",
                    body: body.as_bytes().to_vec(),
                    meta_json: meta,
                    content_hash: hash,
                },
            )
            .await
            .expect("upload");
    }

    /// Upload a sessionless record (a GitHub issue): context for it must fall
    /// back to the record's own chunks.
    async fn put_issue(store: &MemoryStore, external_id: &str, body: &str) {
        let hash = source_meta::hash_body(body.as_bytes());
        let meta = serde_json::json!({
            "source": "github",
            "external_id": external_id,
            "content_hash": hash,
            "title": "index#1: the issue",
            "url": "https://github.com/indexable-inc/index/issues/1",
            "timestamp": 1_781_000_000,
        });
        store
            .upload(
                "s",
                Document {
                    external_id: external_id.to_owned(),
                    file_name: "issue.txt".to_owned(),
                    mime: "text/plain",
                    body: body.as_bytes().to_vec(),
                    meta_json: meta,
                    content_hash: hash,
                },
            )
            .await
            .expect("upload");
    }

    #[tokio::test]
    async fn context_windows_the_session_around_the_anchor() {
        let store = MemoryStore::new();
        for (index, ts) in (1..=7).enumerate() {
            put_turn(&store, "sess-1", &format!("uuid-{index}"), &format!("turn {index}"), ts).await;
        }
        // An unrelated session must never leak into the window.
        put_turn(&store, "sess-2", "other", "other session", 4).await;

        // Anchor at uuid-3 (timestamp 4): two turns either side.
        let view = context(&store, "s", "claude:sess-1:uuid-3", 2, 2)
            .await
            .expect("context");

        let stamps: Vec<i64> = view.turns.iter().filter_map(|turn| turn.timestamp).collect();
        assert_eq!(stamps, vec![2, 3, 4, 5, 6], "ascending window around ts 4");
        assert_eq!(view.anchor, Some(2));
        assert_eq!(
            view.turns[2].external_id.as_deref(),
            Some("claude:sess-1:uuid-3")
        );
        assert!(
            view.turns
                .iter()
                .all(|turn| turn.session_id.as_deref() == Some("sess-1")),
            "no other session leaks in"
        );
    }

    #[tokio::test]
    async fn context_window_clips_at_the_session_edges() {
        let store = MemoryStore::new();
        for (index, ts) in (1..=3).enumerate() {
            put_turn(&store, "sess-1", &format!("uuid-{index}"), &format!("turn {index}"), ts).await;
        }

        // Anchor at the first turn: nothing exists before it, and asking for
        // more after than the session holds returns what is there.
        let view = context(&store, "s", "claude:sess-1:uuid-0", 5, 5)
            .await
            .expect("context");
        let stamps: Vec<i64> = view.turns.iter().filter_map(|turn| turn.timestamp).collect();
        assert_eq!(stamps, vec![1, 2, 3]);
        assert_eq!(view.anchor, Some(0));
    }

    #[tokio::test]
    async fn sessionless_record_falls_back_to_its_own_chunks() {
        let store = MemoryStore::new();
        put_issue(&store, "github:indexable-inc/index#1", "issue body text").await;

        let view = context(&store, "s", "github:indexable-inc/index#1", 3, 3)
            .await
            .expect("context");
        assert_eq!(view.turns.len(), 1);
        assert_eq!(view.anchor, None, "the whole view is the record");
        assert_eq!(view.turns[0].text, "issue body text");
        assert_eq!(view.turns[0].label, "index#1: the issue");
    }

    #[tokio::test]
    async fn bare_session_id_lists_the_session_from_its_start() {
        let store = MemoryStore::new();
        for (index, ts) in (1..=6).enumerate() {
            put_turn(&store, "sess-1", &format!("uuid-{index}"), &format!("turn {index}"), ts).await;
        }

        let view = context(&store, "s", "sess-1", 1, 2).await.expect("context");
        let stamps: Vec<i64> = view.turns.iter().filter_map(|turn| turn.timestamp).collect();
        // before + after + 1 caps the listing; it starts at the session head.
        assert_eq!(stamps, vec![1, 2, 3, 4]);
        assert_eq!(view.anchor, None);
    }

    #[tokio::test]
    async fn unknown_id_is_an_error() {
        let store = MemoryStore::new();
        let err = context(&store, "s", "nope", 2, 2)
            .await
            .expect_err("must not resolve");
        assert!(err.to_string().contains("nope"), "{err}");
    }
}
