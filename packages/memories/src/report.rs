//! The JSON shapes callers build against.
//!
//! Also the row builders behind `stale`, `refuted` and `unchecked`.
//!
//! These structs are the contract. `search` and `show` emit the same hit
//! object, `show` simply without the two ranking keys, so a caller can hand a
//! `show` result to anything that takes a search hit.

use crate::{
    discover::{Corpus, Root},
    error::Result,
    lint::{Diagnostic, UNCHECKED_MAX_DAYS},
    model::{Genre, Memory, Validated},
    rank, stale,
};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// The two ranking numbers, present on a `search` hit and absent on a `show`.
#[derive(Clone, Copy, Debug)]
pub struct Scores {
    pub bm25: f64,
    pub score: f64,
}

/// One memory as JSON. Field order is the contract's order.
#[derive(Debug, Serialize)]
pub struct Hit {
    pub slug: String,
    pub path: String,
    pub root: String,
    pub tldr: String,
    pub genre: Genre,
    pub topic: Vec<String>,
    pub handle: Vec<String>,
    pub prior: f64,
    pub related: Vec<String>,
    pub supersedes: Vec<String>,
    /// `shared`, or `user:<name>`. There is no `always`: nothing is injected
    /// unasked, so a memory has no way to reach a model but a search.
    pub scope: String,
    /// Raw BM25, before the score's multipliers. A caller wanting the unranked
    /// order sorts on this itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bm25: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub stale: bool,
    /// `null`, or a sentence naming what moved. Always present, so a caller
    /// reads one key rather than testing for two shapes.
    pub stale_reason: Option<String>,
    pub refuted: bool,
    pub validated: Vec<Validated>,
    pub body: String,
}

/// One row of the root set: where it is, whether it is there, and how much it
/// held.
///
/// A row rather than a path because "which directories" is not the question a
/// caller has. The question is "did this search cover anything", and neither
/// half answers it alone: the scanned set hides a resolved root that turned out
/// empty, and the resolved set hides whether any of them held anything. Zero
/// hits against `memories: 0` everywhere is unmistakably a coverage problem
/// rather than a genuine miss.
#[derive(Debug, Serialize)]
pub struct RootRow {
    pub path: String,
    /// Whether the `.memories` directory is on disk. A resolved root that is not
    /// there is still reported: "we looked here" is what the caller needs.
    pub exists: bool,
    /// Memory files read from it, parsed or not.
    pub memories: usize,
}

#[derive(Debug, Serialize)]
pub struct SearchOutput {
    pub query: String,
    /// The root set this query read, one row each.
    ///
    /// Reported with every result on purpose: an empty result from a root set
    /// that resolved to one unexpected directory is indistinguishable from an
    /// empty result from the right directories, and that is how a search tool
    /// silently stops working.
    pub roots: Vec<RootRow>,
    /// Memory files read, whether or not they parsed.
    pub scanned: usize,
    pub elapsed_ms: u128,
    pub hits: Vec<Hit>,
}

/// What `memories roots` emits: the same rows `search` reports, from the same
/// function, so the two spellings of one idea cannot drift.
#[derive(Debug, Serialize)]
pub struct RootsOutput {
    pub roots: Vec<RootRow>,
}

#[derive(Debug, Serialize)]
pub struct LintOutput {
    pub diagnostics: Vec<Diagnostic>,
    pub errors: usize,
    pub checked: usize,
}

/// One row of `stale`, `refuted` or `unchecked`.
#[derive(Debug, Serialize)]
pub struct Row {
    pub slug: String,
    pub path: String,
    pub tldr: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct RowsOutput {
    pub rows: Vec<Row>,
}

/// Render one memory, checking its `based_on` hashes as it goes.
///
/// # Errors
///
/// Returns an error when a `based_on` file exists but cannot be read.
pub fn hit(memory: &Memory, scores: Option<Scores>) -> Result<Hit> {
    let staleness = stale::check(memory)?;
    Ok(Hit {
        slug: memory.slug.clone(),
        path: memory.path.display().to_string(),
        root: memory.root.display().to_string(),
        tldr: memory.tldr.clone(),
        genre: memory.genre,
        topic: memory.topic.clone(),
        handle: memory.handle.clone(),
        prior: memory.prior,
        related: memory.related.clone(),
        supersedes: memory.supersedes.clone(),
        scope: memory.scope.rendered(),
        bm25: scores.map(|scores| scores.bm25),
        score: scores.map(|scores| scores.score),
        stale: staleness.stale,
        stale_reason: staleness.reason,
        refuted: memory.is_refuted(),
        validated: memory.validated.clone(),
        body: memory.body.clone(),
    })
}

fn row(memory: &Memory, reason: String) -> Row {
    Row {
        slug: memory.slug.clone(),
        path: memory.path.display().to_string(),
        tldr: memory.tldr.clone(),
        reason,
    }
}

/// Every memory whose `based_on` no longer matches. Refuted memories are
/// included: a memory can be both, and hiding one flag behind the other loses
/// information.
///
/// # Errors
///
/// Returns an error when a `based_on` file exists but cannot be read.
pub fn stale_rows(corpus: &Corpus) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for memory in &corpus.memories {
        let staleness = stale::check(memory)?;
        if let Some(reason) = staleness.reason {
            rows.push(row(memory, reason));
        }
    }
    Ok(rows)
}

/// Every memory whose newest validation says it did not hold.
#[must_use]
pub fn refuted_rows(corpus: &Corpus) -> Vec<Row> {
    corpus
        .memories
        .iter()
        .filter(|memory| memory.is_refuted())
        .map(|memory| {
            let reason = memory.newest_validation().map_or_else(
                || "refuted".to_owned(),
                |entry| {
                    format!(
                        "refuted {at} by {by}: {how}",
                        at = entry.at,
                        by = entry.by,
                        how = entry.how
                    )
                },
            );
            row(memory, reason)
        })
        .collect()
}

/// Every `genre: memory` nobody has validated inside `days`.
///
/// Scoped to that genre for the same reason `memory-unchecked` is: a reference
/// page is supposed to be long-lived, and a validation clock on one produces a
/// wall of rows that says nothing. That scoping is what replaces an `evergreen`
/// escape hatch.
#[must_use]
pub fn unchecked_rows(corpus: &Corpus, now: DateTime<Utc>, days: f64) -> Vec<Row> {
    corpus
        .memories
        .iter()
        .filter(|memory| memory.genre == crate::model::Genre::Memory)
        .filter_map(|memory| {
            memory.newest_validation().map_or_else(
                || Some(row(memory, "never validated".to_owned())),
                |newest| {
                    let age_days = rank::days_between(newest.at_time, now);
                    (age_days > days).then(|| {
                        row(
                            memory,
                            format!("last validated {age_days:.0} days ago, over {days:.0}"),
                        )
                    })
                },
            )
        })
        .collect()
}

/// The root set as reported rows, in precedence order.
///
/// One function for both `search --json` and `memories roots`: two spellings of
/// one idea is the drift this field exists to prevent.
#[must_use]
pub fn root_rows(roots: &[Root], corpus: &Corpus) -> Vec<RootRow> {
    roots
        .iter()
        .map(|root| {
            // Only an existing directory gets a scan, so a missing row is a
            // missing directory rather than an empty one.
            let scan = corpus
                .scans
                .iter()
                .find(|scan| scan.root.memories_dir == root.memories_dir);
            RootRow {
                path: root.memories_dir.display().to_string(),
                exists: scan.is_some(),
                memories: scan.map_or(0, |scan| scan.leaves.iter().map(|leaf| leaf.files).sum()),
            }
        })
        .collect()
}

/// The `unchecked` window when the caller does not name one.
#[must_use]
pub const fn default_unchecked_days() -> f64 {
    UNCHECKED_MAX_DAYS
}
