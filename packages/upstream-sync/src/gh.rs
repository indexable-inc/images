//! `gh`-backed PR reads and duplicate search, plus the drift report's
//! degrade-instead-of-fail forge reads.

use anstream::eprintln;
use color_eyre::eyre::Result;
use serde::Deserialize;

use crate::cmd;
use crate::mapping::Slug;
use crate::status::{Duplicate, Pr, utc_stamp};
use crate::style::{YELLOW, paint};

/// Refresh a tracked PR's live state, or `None` if the PR can no longer be
/// read (deleted/renamed).
///
/// The result's `state` collapses gh's separate `state` (OPEN/CLOSED/MERGED)
/// and `isDraft` into one of open|draft|merged|closed.
///
/// # Errors
/// Fails only when `gh` cannot be spawned.
pub fn refresh_pr(slug: &Slug, number: u64) -> Result<Option<Pr>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct View {
        state: String,
        is_draft: bool,
        url: String,
        number: u64,
    }

    let res = cmd::complete(
        "gh",
        &[
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &format!("{}/{}", slug.owner, slug.repo),
            "--json",
            "state,isDraft,url,number",
        ],
    )?;
    if !res.ok() {
        return Ok(None);
    }
    let Ok(view) = serde_json::from_str::<View>(&res.stdout) else {
        return Ok(None);
    };
    let state = match view.state.as_str() {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ if view.is_draft => "draft",
        _ => "open",
    };
    Ok(Some(Pr {
        url: view.url,
        number: view.number,
        state: state.to_owned(),
        checked_at: utc_stamp(),
    }))
}

/// Distinctive lowercase tokens of a patch subject.
///
/// Alphanumerics, min length 4, minus generic contribution/domain filler
/// that would match everything. Used to build a tight duplicate query and to
/// post-filter gh's fuzzy hits.
#[must_use]
pub fn subject_tokens(subject: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "add", "fix", "the", "and", "for", "with", "from", "into", "when", "test", "tests",
        "doc", "docs", "note", "feature", "command", "support", "allow", "make", "use",
        "libstore", "libutil", "libexpr", "nix", "build", "status",
    ];
    let lower = subject.to_lowercase();
    let mut seen: Vec<String> = Vec::new();
    for token in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
        if token.len() >= 4 && !STOP.contains(&token) && !seen.iter().any(|s| s == token) {
            seen.push(token.to_owned());
        }
    }
    seen
}

/// Search the upstream repo for OPEN PRs that plausibly DUPLICATE this
/// patch, to record and skip rather than open a competing one.
///
/// gh's PR search is a fuzzy OR over tokens, so we (1) query only
/// distinctive title tokens with `in:title`, then (2) post-filter to hits
/// whose title shares at least 2 of our distinctive tokens. This trades a
/// few missed near-matches for far fewer false positives (the skip is
/// conservative-safe: a real dup we miss just gets an extra PR a human can
/// dedupe, whereas a false dup that BLOCKS an attempt is a silent no-op we
/// do NOT want). Best-effort: any failure or a tokenless subject returns
/// `[]` so the loop never stalls.
///
/// # Errors
/// Fails only when `gh` cannot be spawned.
pub fn find_duplicates(slug: &Slug, subject: &str) -> Result<Vec<Duplicate>> {
    let tokens = subject_tokens(subject);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let query = format!("{} in:title", tokens.join(" "));
    let res = cmd::complete(
        "gh",
        &[
            "search",
            "prs",
            &query,
            "--repo",
            &format!("{}/{}", slug.owner, slug.repo),
            "--state",
            "open",
            "--limit",
            "20",
            "--json",
            "url,number,title",
        ],
    )?;
    if !res.ok() {
        return Ok(Vec::new());
    }
    let Ok(hits) = serde_json::from_str::<Vec<Duplicate>>(&res.stdout) else {
        return Ok(Vec::new());
    };
    Ok(hits
        .into_iter()
        .filter(|hit| {
            let hit_tokens = subject_tokens(&hit.title);
            tokens.iter().filter(|t| hit_tokens.contains(t)).count() >= 2
        })
        .collect())
}

/// A `gh api` read that DEGRADES instead of failing: a forge error becomes a
/// stderr warning and an unknown cell, so one unreachable forge cannot take
/// the whole drift table down.
///
/// # Errors
/// Fails only when `gh` cannot be spawned.
pub fn read(ctx: &str, path: &str, jq: &str) -> Result<Option<String>> {
    let res = cmd::complete("gh", &["api", path, "--jq", jq])?;
    if !res.ok() {
        eprintln!(
            "{}",
            paint(YELLOW, &format!("upstream-sync: drift: {ctx}: gh api {path} failed; cell left unknown"))
        );
        return Ok(None);
    }
    Ok(Some(res.stdout.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_drop_stopwords_short_words_and_dupes() {
        let t = subject_tokens("fix libstore: don't crash the daemon when a daemon GC-roots scan races");
        assert_eq!(t, vec!["crash", "daemon", "roots", "scan", "races"]);
    }

    #[test]
    fn tokenless_subject_yields_empty() {
        assert!(subject_tokens("fix the a of").is_empty());
    }
}
