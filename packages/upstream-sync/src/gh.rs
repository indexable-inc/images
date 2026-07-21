//! `gh`-backed PR reads and duplicate search, plus the drift report's
//! degrade-instead-of-fail forge reads.

use anstream::eprintln;
use color_eyre::eyre::Result;
use serde::Deserialize;

use crate::cmd;
use crate::mapping::Slug;
use crate::status::{Duplicate, Pr, utc_stamp};
use crate::style::{YELLOW, paint};

/// One entry of gh's `statusCheckRollup`: a mix of `CheckRun` entries
/// carrying name/status/conclusion and commit-status contexts carrying
/// context/state. Every field is optional because each kind carries only its
/// own subset.
#[derive(Debug, Default, Deserialize)]
struct RollupItem {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

/// The collapsed upstream CI verdict of one PR head, with the failing check
/// names kept so a red verdict is actionable without opening the PR.
struct CiVerdict {
    /// failing | pending | passing | none.
    verdict: &'static str,
    /// Names of the failing checks (check-run `name` or status `context`).
    failing: Vec<String>,
}

/// Collapse a `statusCheckRollup` into one verdict. CANCELLED counts as
/// failing: on fail-fast matrices (nushell) the cancelled jobs ride along
/// with a real failure, and a required check that was cancelled is not green
/// either way.
fn ci_verdict(rollup: &[RollupItem]) -> CiVerdict {
    const FAILING_CONCLUSIONS: [&str; 5] = [
        "FAILURE",
        "TIMED_OUT",
        "CANCELLED",
        "ACTION_REQUIRED",
        "STARTUP_FAILURE",
    ];
    const FAILING_STATES: [&str; 2] = ["FAILURE", "ERROR"];
    let is_failing = |item: &RollupItem| {
        item.conclusion
            .as_deref()
            .is_some_and(|c| FAILING_CONCLUSIONS.contains(&c))
            || item
                .state
                .as_deref()
                .is_some_and(|s| FAILING_STATES.contains(&s))
    };
    let is_pending = |item: &RollupItem| {
        item.status
            .as_deref()
            .is_some_and(|s| !s.is_empty() && s != "COMPLETED")
            || item
                .state
                .as_deref()
                .is_some_and(|s| s == "PENDING" || s == "EXPECTED")
    };
    let failing: Vec<String> = rollup
        .iter()
        .filter(|item| is_failing(item))
        .map(|item| {
            item.name
                .clone()
                .or_else(|| item.context.clone())
                .unwrap_or_else(|| "unknown".to_owned())
        })
        .collect();
    let verdict = if rollup.is_empty() {
        "none"
    } else if failing.is_empty() {
        if rollup.iter().any(is_pending) {
            "pending"
        } else {
            "passing"
        }
    } else {
        "failing"
    };
    CiVerdict { verdict, failing }
}

/// Refresh a tracked PR's live state, or `None` if the PR can no longer be
/// read (deleted/renamed).
///
/// The result's `state` collapses gh's separate `state` (OPEN/CLOSED/MERGED)
/// and `isDraft` into one of open|draft|merged|closed; its `ci` collapses the
/// head commit's `statusCheckRollup` into failing | pending | passing | none
/// (see [`ci_verdict`]), with the failing check names kept.
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
        #[serde(default)]
        status_check_rollup: Option<Vec<RollupItem>>,
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
            "state,isDraft,url,number,statusCheckRollup",
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
    let ci = ci_verdict(&view.status_check_rollup.unwrap_or_default());
    Ok(Some(Pr {
        url: view.url,
        number: view.number,
        state: state.to_owned(),
        ci: ci.verdict.to_owned(),
        failing_checks: ci.failing,
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
        "add", "fix", "the", "and", "for", "with", "from", "into", "when", "test", "tests", "doc",
        "docs", "note", "feature", "command", "support", "allow", "make", "use", "libstore",
        "libutil", "libexpr", "nix", "build", "status",
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
            paint(
                YELLOW,
                &format!("upstream-sync: drift: {ctx}: gh api {path} failed; cell left unknown")
            )
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
        let t = subject_tokens(
            "fix libstore: don't crash the daemon when a daemon GC-roots scan races",
        );
        assert_eq!(t, vec!["crash", "daemon", "roots", "scan", "races"]);
    }

    #[test]
    fn tokenless_subject_yields_empty() {
        assert!(subject_tokens("fix the a of").is_empty());
    }

    fn rollup(raw: &str) -> Vec<RollupItem> {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn ci_verdict_collapses_the_rollup() {
        // Empty rollup: no CI configured upstream.
        assert_eq!(ci_verdict(&[]).verdict, "none");

        // A failure wins over green and pending; CANCELLED counts as failing
        // (fail-fast matrices cancel the rest of the wall); failing names
        // come from the check-run `name` or the status `context`.
        let red = ci_verdict(&rollup(
            r#"[{"name":"cargo fmt","status":"COMPLETED","conclusion":"FAILURE"},
                {"name":"cargo test","status":"COMPLETED","conclusion":"CANCELLED"},
                {"context":"ci/other","state":"SUCCESS"},
                {"name":"docs","status":"IN_PROGRESS"}]"#,
        ));
        assert_eq!(red.verdict, "failing");
        assert_eq!(red.failing, vec!["cargo fmt", "cargo test"]);

        // No failure but an unfinished check-run or a pending context.
        let pending = ci_verdict(&rollup(
            r#"[{"name":"build","status":"IN_PROGRESS"},
                {"context":"ci/other","state":"SUCCESS"}]"#,
        ));
        assert_eq!(pending.verdict, "pending");
        assert!(pending.failing.is_empty());
        assert_eq!(
            ci_verdict(&rollup(r#"[{"context":"ci/x","state":"EXPECTED"}]"#)).verdict,
            "pending"
        );

        // Everything completed green.
        let green = ci_verdict(&rollup(
            r#"[{"name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                {"context":"ci/other","state":"SUCCESS"}]"#,
        ));
        assert_eq!(green.verdict, "passing");
    }
}
