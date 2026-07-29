//! `gh`-backed PR reads and duplicate search, plus the drift report's
//! degrade-instead-of-fail forge reads.

use anstream::eprintln;
use color_eyre::eyre::{Result, eyre};
use lazy_regex::regex;
use serde::Deserialize;

use crate::cmd;
use crate::mapping::Slug;
use crate::status::{Checks, Duplicate, Pr, utc_stamp};
use crate::style::{YELLOW, paint};

/// One entry of a PR's check rollup.
///
/// The rollup mixes two node types: Actions check runs carry `status` plus
/// `conclusion`, older commit statuses carry `state`. Every field is
/// optional so either kind deserializes into this one shape.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RollupEntry {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

/// Refresh a tracked PR's live state, or `None` if the PR is genuinely gone
/// (deleted, renamed, or the repo no longer resolves).
///
/// The result's `state` collapses gh's separate `state` (OPEN/CLOSED/MERGED)
/// and `isDraft` into one of open|draft|merged|closed.
///
/// `None` is reserved for the forge ANSWERING that the PR is not there, and
/// nothing else. It used to mean "the read failed for any reason at all",
/// and the difference is not academic: in run 30443012384 an expired
/// `UPSTREAM_PR_TOKEN` made every read return `HTTP 401: Bad credentials`,
/// and the caller wrote "tracked PR #9718 no longer readable, deleted or
/// renamed" into the committed status file for two PRs that were, and are,
/// open. A wrong fact recorded silently is worse than a red run, so a read
/// that failed for any reason other than "not there" is now fatal.
///
/// # Errors
/// Fails when `gh` cannot be spawned, and when the read failed for any
/// reason other than the PR being absent.
pub fn refresh_pr(slug: &Slug, number: u64) -> Result<Option<Pr>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct View {
        state: String,
        is_draft: bool,
        url: String,
        number: u64,
        #[serde(default)]
        status_check_rollup: Vec<RollupEntry>,
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
        // gh answers a missing PR or repo with GraphQL's own resolution
        // error; every other failure (401, 403, 5xx, no network) says
        // nothing about whether the PR exists.
        if res.stderr.contains("Could not resolve to a") {
            return Ok(None);
        }
        return Err(eyre!(
            "upstream-sync: cannot read {}/{}#{number}: {}. Refusing to record this as a \
             deleted PR: the forge did not say the PR is gone, it failed to answer.",
            slug.owner,
            slug.repo,
            res.stderr.trim()
        ));
    }
    let view = serde_json::from_str::<View>(&res.stdout).map_err(|err| {
        eyre!(
            "upstream-sync: {}/{}#{number}: gh returned unparseable JSON: {err}",
            slug.owner,
            slug.repo
        )
    })?;
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
        checks: tally(&view.status_check_rollup),
        checked_at: utc_stamp(),
    }))
}

/// Fold a check rollup into counts, or `None` when the PR has no checks.
///
/// A check run that is queued or running counts as pending; a commit status
/// with no verdict yet reads the same way. NEUTRAL, SKIPPED and CANCELLED
/// count as neither pass nor fail: a cancelled job says nothing about the
/// change, and counting it red would make every superseded run look like a
/// regression.
fn tally(entries: &[RollupEntry]) -> Option<Checks> {
    if entries.is_empty() {
        return None;
    }
    let mut checks = Checks {
        passing: 0,
        failing: 0,
        pending: 0,
    };
    for entry in entries {
        let running = matches!(entry.status.as_deref(), Some("QUEUED" | "IN_PROGRESS"));
        let verdict = entry
            .conclusion
            .as_deref()
            .or(entry.state.as_deref())
            .unwrap_or("");
        match verdict {
            _ if running => checks.pending += 1,
            "SUCCESS" => checks.passing += 1,
            "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
                checks.failing += 1;
            }
            "PENDING" | "EXPECTED" | "" => checks.pending += 1,
            _ => {}
        }
    }
    Some(checks)
}

/// The PR already open FROM OUR OWN fork branch for this patch, if any.
///
/// Asked before the duplicate search, because "have we already sent this?"
/// and "did someone else propose this?" are different questions and the
/// fuzzy search cannot tell them apart. It could not: on 2026-07-27 a PR
/// opened minutes earlier by `upstream-pr --open` came back from
/// [`find_duplicates`] as a competing PR, and the patch was skipped as a
/// duplicate of itself with `pr: null` left in the status file.
///
/// The head branch plus its owning repo is the identity. `upstream-pr`
/// derives the branch from the patch subject, so it names this patch and no
/// other. `gh pr list --head` matches on the bare branch NAME and silently
/// returns nothing for an `owner:branch` argument, so the owner is checked
/// here instead: without that check, an unrelated contributor pushing a
/// branch of the same name would be adopted as ours.
///
/// # Errors
/// Fails when `gh` cannot be spawned, and when the list read fails. A
/// swallowed failure here answers "we have no PR for this patch", and the
/// caller acts on that by opening one -- on a repo we do not own. An
/// unreadable list is not evidence of absence, and the cost of guessing
/// wrong is an outward act we cannot take back.
pub fn find_ours(slug: &Slug, fork_owner: &str, branch: &str) -> Result<Option<Pr>> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listed {
        number: u64,
        url: String,
        state: String,
        is_draft: bool,
        head_repository_owner: Option<Owner>,
    }

    #[derive(Deserialize)]
    struct Owner {
        login: String,
    }

    let res = cmd::complete(
        "gh",
        &[
            "pr",
            "list",
            "--repo",
            &format!("{}/{}", slug.owner, slug.repo),
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number,url,state,isDraft,headRepositoryOwner",
        ],
    )?;
    if !res.ok() {
        return Err(eyre!(
            "upstream-sync: cannot list PRs from {}/{} head {branch}: {}. Refusing to treat an \
             unreadable list as \"no PR of ours\": the next step would open a duplicate on a \
             repo we do not own.",
            slug.owner,
            slug.repo,
            res.stderr.trim()
        ));
    }
    let hits = serde_json::from_str::<Vec<Listed>>(&res.stdout).map_err(|err| {
        eyre!(
            "upstream-sync: {}/{} head {branch}: gh returned unparseable JSON: {err}",
            slug.owner,
            slug.repo
        )
    })?;
    let mut hits: Vec<Listed> = hits
        .into_iter()
        .filter(|h| {
            h.head_repository_owner
                .as_ref()
                .is_some_and(|o| o.login.eq_ignore_ascii_case(fork_owner))
        })
        .collect();
    // Newest first: a re-pushed branch can carry a closed PR and a later
    // open one, and the live one is the one to track.
    hits.sort_by_key(|h| std::cmp::Reverse(h.number));
    Ok(hits.into_iter().next().map(|h| Pr {
        url: h.url,
        number: h.number,
        state: match h.state.as_str() {
            "MERGED" => "merged".to_owned(),
            "CLOSED" => "closed".to_owned(),
            _ if h.is_draft => "draft".to_owned(),
            _ => "open".to_owned(),
        },
        // Filled in by the next refresh; this call does not read the rollup.
        checks: None,
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

/// The HTTP status `gh` reports in its own error line, e.g.
/// `gh: No commit found for SHA: abc (HTTP 422)`. `None` when the failure
/// never reached HTTP at all (DNS, TLS, connection refused), which is the
/// most unreachable case of the lot.
fn http_status(stderr: &str) -> Option<u16> {
    regex!(r"\(HTTP (\d{3})\)")
        .captures(stderr)
        .and_then(|c| c[1].parse().ok())
}

/// A forge answered, and the answer was that the object is not there.
///
/// Kept apart from an unreachable forge because the two want opposite
/// handling, and conflating them is what made ENG-11160 unreadable: the
/// operator was told "unreachable" about a forge that had replied in
/// milliseconds with a precise reason.
pub struct Absent {
    pub status: u16,
    /// The forge's own words, e.g. `No commit found for SHA: c1b4a88a...`.
    pub detail: String,
}

/// The outcome of a drift probe: a value, or a definitive "not here".
pub enum Read {
    Value(String),
    Absent(Absent),
}

/// A `gh api` read for the drift report.
///
/// 404 and 422 are answers, not failures: the forge is up and is telling us
/// something durable about our own pin (a rev the upstream repo has never
/// heard of, because the fork shares no object store with it, or because the
/// pin was GC'd). Those degrade to an unknown cell, which is the documented
/// contract -- the table must still render its other rows.
///
/// Everything else is fatal, and deliberately so. A 401, a 403 or a 502
/// yields no information whatsoever, and a drift table built from no
/// information does not read as "unknown", it reads as "no drift". That is
/// the silent-success failure mode, and it is worse than a red run: run
/// 30443012384 carried a `notify` step dying on `HTTP 401: Bad credentials`
/// against the same API this probe uses, so an expired token is a live
/// possibility, not a hypothetical.
///
/// # Errors
/// Fails when `gh` cannot be spawned, and when the forge could not be
/// reached or refused the read.
pub fn read(ctx: &str, path: &str, jq: &str) -> Result<Read> {
    let res = cmd::complete("gh", &["api", path, "--jq", jq])?;
    if res.ok() {
        return Ok(Read::Value(res.stdout.trim().to_owned()));
    }
    let detail = res.stderr.trim().to_owned();
    let status = http_status(&detail);
    if !matches!(status, Some(404 | 422)) {
        return Err(eyre!(
            "upstream-sync: drift: {ctx}: cannot reach the forge for `gh api {path}`: {}. \
             This is fatal rather than an unknown cell: a drift table computed without the \
             forge reads as \"no drift\", not as \"unknown\". Check GH_TOKEN and the forge's \
             status, then re-run.",
            if detail.is_empty() {
                format!("gh exited {}", res.status)
            } else {
                detail.clone()
            }
        ));
    }
    let absent = Absent {
        // Checked immediately above.
        status: status.unwrap_or_default(),
        detail,
    };
    eprintln!(
        "{}",
        paint(
            YELLOW,
            &format!(
                "upstream-sync: drift: {ctx}: `gh api {path}` answered HTTP {}: {}. The pinned \
                 rev is not present upstream -- either the fork repo is not a GitHub fork of the \
                 upstream (they share no object store, so a megamerge sha can never resolve \
                 there) or the rev was garbage-collected. Cell left unknown.",
                absent.status, absent.detail
            )
        )
    );
    Ok(Read::Absent(absent))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real stderr of the two reads that went unexplained for four days
    // (ENG-11160), plus the credential failure that must NOT be mistaken for
    // one of them.
    #[test]
    fn http_status_is_read_from_ghs_own_error_line() {
        assert_eq!(
            http_status("gh: No commit found for SHA: c1b4a88a (HTTP 422)"),
            Some(422)
        );
        assert_eq!(http_status("gh: Not Found (HTTP 404)"), Some(404));
        assert_eq!(http_status("gh: Bad credentials (HTTP 401)"), Some(401));
        // A failure that never reached HTTP carries no status, and so falls
        // on the fatal side rather than being read as a missing object.
        assert_eq!(
            http_status("dial tcp: lookup api.github.com: no such host"),
            None
        );
    }

    #[test]
    fn only_404_and_422_degrade_to_an_unknown_cell() {
        let degrades = |stderr: &str| matches!(http_status(stderr), Some(404 | 422));
        assert!(degrades("gh: Not Found (HTTP 404)"));
        assert!(degrades("gh: No commit found for SHA: c1b4a88a (HTTP 422)"));
        // Credentials and forge outages yield no information, so a drift
        // table built from them would read as "no drift".
        assert!(!degrades("gh: Bad credentials (HTTP 401)"));
        assert!(!degrades("gh: Forbidden (HTTP 403)"));
        assert!(!degrades("gh: Bad Gateway (HTTP 502)"));
        assert!(!degrades("connection refused"));
    }

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

    fn entry(status: Option<&str>, conclusion: Option<&str>, state: Option<&str>) -> RollupEntry {
        RollupEntry {
            status: status.map(ToOwned::to_owned),
            conclusion: conclusion.map(ToOwned::to_owned),
            state: state.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn no_checks_is_distinct_from_all_green() {
        assert!(tally(&[]).is_none());
        let green = tally(&[entry(Some("COMPLETED"), Some("SUCCESS"), None)]).unwrap();
        assert_eq!((green.passing, green.failing), (1, 0));
        assert!(!green.red());
    }

    #[test]
    fn cancelled_is_not_red_but_failure_is() {
        // The real shape of nushell#18549: two failures, several cancelled
        // runs superseded by them, and some passes.
        let t = tally(&[
            entry(Some("COMPLETED"), Some("FAILURE"), None),
            entry(Some("COMPLETED"), Some("FAILURE"), None),
            entry(Some("COMPLETED"), Some("SUCCESS"), None),
            entry(Some("COMPLETED"), Some("CANCELLED"), None),
            entry(Some("IN_PROGRESS"), None, None),
            entry(None, None, Some("PENDING")),
        ])
        .unwrap();
        assert_eq!((t.passing, t.failing, t.pending), (1, 2, 2));
        assert!(t.red());
        assert_eq!(t.summary(), "1 passing, 2 failing, 2 pending");
    }
}
