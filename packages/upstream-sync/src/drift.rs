//! The read-only drift companion report (RFC 0010, #2098).
//!
//! Per fork: how far the pinned base trails its tracked upstream branch
//! (the registry's `upstreamRef`, else the default branch; commits behind +
//! base age), the declared patch stances, how many tracked
//! patches are retired-awaiting-drop, and a one-word next action. The
//! fork-sync cron surfaces it in its step summary and rolling PR body.
//!
//! A missing cell means "unknown" (forge unreachable or input not in
//! flake.lock), never a crash: the report must survive a broken forge and
//! still render the other rows.

use std::fs;
use std::path::Path;

use anstream::eprintln;
use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, WrapErr, eyre};
use lazy_regex::regex;
use serde::Serialize;

use crate::gh;
use crate::mapping::{self, Fork, Slug};
use crate::report;
use crate::status;
use crate::style::{YELLOW, paint};

/// What a 404 or 422 means for this lane, for [`gh::read`]'s warning. The probe
/// is aimed at the UPSTREAM repo, which shares no object store with a
/// mesa-style fork, so a megamerge sha resolving to nothing there is an expected
/// answer rather than a broken state.
const ABSENT_HINT: &str = "The pinned rev is not present upstream -- either the fork repo is not \
     a GitHub fork of the upstream (they share no object store, so a megamerge sha can never \
     resolve there) or the rev was garbage-collected. Cell left unknown.";

/// One fork's drift facts (the `--json` row shape).
#[derive(Debug, Serialize)]
pub struct Row {
    pub name: String,
    pub forge: String,
    pub input: String,
    pub rev: Option<String>,
    pub behind: Option<i64>,
    #[serde(rename = "baseDate")]
    pub base_date: Option<String>,
    #[serde(rename = "ageDays")]
    pub age_days: Option<i64>,
    pub attempt: usize,
    pub hold: usize,
    pub never: usize,
    pub rejected: usize,
    pub retired: usize,
    pub action: String,
    pub note: String,
}

/// Render the report. Network reads only; no status file is written.
///
/// # Errors
/// Fails on flag conflicts, an unknown fork name, or unreadable local data
/// (mapping, flake.lock, status file); forge errors degrade to unknown cells.
pub fn run(
    mapping_override: Option<&Path>,
    name: Option<&str>,
    json: bool,
    markdown: bool,
) -> Result<()> {
    report::check_flags("drift", json, markdown)?;
    let mapping_path = mapping::path(mapping_override)?;
    let forks = mapping::select(mapping::load(&mapping_path)?, name, "upstream-sync")?;
    let rows = forks.iter().map(row).collect::<Result<Vec<Row>>>()?;
    report::emit(
        &rows,
        &render_table(&rows),
        "== fork drift: pinned base vs upstream default branch ==",
        json,
        markdown,
    )
}

/// The pinned base rev of a fork's input from the committed flake.lock in
/// the CWD (the tool runs from the repo root; a downstream --mapping repo
/// reads its own lock the same way). `None` when the lock or input is
/// absent.
///
/// # Errors
/// Fails when an existing flake.lock is unreadable or not JSON.
pub(crate) fn lock_rev(input: &str) -> Result<Option<String>> {
    let lock = Path::new("flake.lock");
    if !lock.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(lock).wrap_err("cannot read flake.lock")?;
    let v: serde_json::Value = serde_json::from_str(&raw).wrap_err("cannot parse flake.lock")?;
    Ok(v.get("nodes")
        .and_then(|nodes| nodes.get(input))
        .and_then(|node| node.get("locked"))
        .and_then(|locked| locked.get("rev"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

/// Commits behind = `ahead_by` of `pinned...branch`: how many commits the
/// tracked upstream branch (the registry's `upstreamRef`, else the default
/// branch) has that our pinned base does not.
fn github_behind(fork: &Fork, slug: &Slug, rev: &str) -> Result<Option<i64>> {
    let ctx = format!("drift: {}", fork.name);
    let repo = format!("repos/{}/{}", slug.owner, slug.repo);
    let branch = match &fork.upstream_ref {
        Some(configured) => configured.clone(),
        None => match gh::read(&ctx, &repo, ".default_branch", ABSENT_HINT)? {
            gh::Read::Value(branch) => branch,
            gh::Read::Absent(_) => return Ok(None),
        },
    };
    let gh::Read::Value(n) = gh::read(
        &ctx,
        &format!("{repo}/compare/{rev}...{branch}"),
        ".ahead_by",
        ABSENT_HINT,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(n.parse().wrap_err_with(|| {
        format!("gh compare ahead_by is not a number: {n}")
    })?))
}

/// Base-commit committer date on a GitLab host, or `None`. GitLab drift is
/// DELIBERATELY base-age-only: the compare API enumerates every commit in
/// the range (thousands on a months-old mesa pin), so the cheap
/// single-commit lookup is the reliable unauthenticated read and
/// commits-behind stays unknown (the RFC allows exactly this degradation).
///
/// # Errors
/// Fails when the GitLab host cannot be reached or refuses the read, on the
/// same reasoning as [`gh::read`]: no answer is not the same as the answer
/// "not here", and only the latter may become an unknown cell.
fn gitlab_base_date(name: &str, url: &str, rev: &str) -> Result<Option<String>> {
    let Some(caps) = regex!(r"^https?://([^/]+)/(.+)$").captures(url) else {
        return Ok(None);
    };
    let host = &caps[1];
    let path = caps[2].trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    // The API wants the project path as ONE path segment, so the separating
    // slash has to be %2F. Only the slash: the nu predecessor used `url
    // encode --all` and encoded every byte, which is equally legal (verified:
    // `projects/%6D%65%73%61%2F%6D%65%73%61` returns the same 200 as
    // `projects/mesa%2Fmesa`) but rendered the endpoint unreadable in exactly
    // the error messages someone debugging is reading. ENG-11160 was misread
    // as an encoding bug for that reason alone.
    let project = path.replace('/', "%2F");
    let endpoint = format!("https://{host}/api/v4/projects/{project}/repository/commits/{rev}");

    let mut res = match ureq::get(&endpoint).call() {
        Ok(res) => res,
        // ureq surfaces a non-2xx as an error carrying the response, so an
        // answered 404 has to be dug back out before it is called a
        // connection problem.
        Err(ureq::Error::StatusCode(code)) if matches!(code, 404 | 422) => {
            eprintln!(
                "{}",
                paint(
                    YELLOW,
                    &format!(
                        "upstream-sync: drift: {name}: {endpoint} answered HTTP {code}. The \
                         pinned rev is not present upstream -- mesa-style forks live on GitHub \
                         while the upstream is GitLab, so the two share no object store and a \
                         megamerge sha can never resolve there. Base age left unknown."
                    )
                )
            );
            return Ok(None);
        }
        Err(err) => {
            return Err(eyre!(
                "upstream-sync: drift: {name}: cannot reach {endpoint}: {err}. This is fatal \
                 rather than an unknown cell: a drift table computed without the forge reads as \
                 \"no drift\", not as \"unknown\"."
            ));
        }
    };
    let v = res
        .body_mut()
        .read_json::<serde_json::Value>()
        .wrap_err_with(|| format!("upstream-sync: drift: {name}: {endpoint} is not JSON"))?;
    Ok(v.get("committed_date")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

fn base_date(fork: &Fork, forge: &str, slug: &Slug, rev: Option<&str>) -> Result<Option<String>> {
    let Some(rev) = rev else { return Ok(None) };
    match forge {
        "github" => gh::read(
            &format!("drift: {}", fork.name),
            &format!("repos/{}/{}/commits/{rev}", slug.owner, slug.repo),
            ".commit.committer.date",
            ABSENT_HINT,
        )
        .map(|read| match read {
            gh::Read::Value(date) => Some(date),
            gh::Read::Absent(_) => None,
        }),
        "gitlab" => gitlab_base_date(&fork.name, &fork.upstream_url, rev),
        _ => Ok(None),
    }
}

fn age_days(date: &str) -> Result<i64> {
    let dt = DateTime::parse_from_rfc3339(date)
        .wrap_err_with(|| format!("unparseable base date {date}"))?;
    Ok(Utc::now()
        .signed_duration_since(dt)
        .num_seconds()
        .div_euclid(86_400))
}

/// Patch stances count the declared intent map (subject-keyed). The report
/// is deliberately clone-free, so an unclassified series commit (present in
/// the fork repo, absent here) is not counted; the sync loop, which does
/// open the fork repo, is where unclassified patches surface as hold.
struct StanceCounts {
    attempt: usize,
    hold: usize,
    never: usize,
    rejected: usize,
}

fn stance_counts(fork: &Fork) -> StanceCounts {
    let stances: Vec<String> = fork.patches.keys().map(|s| fork.stance(s)).collect();
    let count = |wanted: &str| stances.iter().filter(|s| *s == wanted).count();
    StanceCounts {
        attempt: count("attempt"),
        hold: count("hold"),
        never: count("never"),
        rejected: count("rejected"),
    }
}

fn row(fork: &Fork) -> Result<Row> {
    let slug = Slug::parse(&fork.upstream_url)?;
    // A vendored fork has no pinned base to measure upstream distance FROM, so
    // this lane cannot answer its question for one. Its cells stay unknown and
    // its `input` cell says `vendored:<path>`, rather than an empty row that
    // reads as no drift. Getting the answer back means deriving the view
    // locally to find the base it sits on, which this network-only lane does
    // not do; ENG-11685 owns that.
    let rev = match fork.source()? {
        mapping::Source::Pinned(input) => {
            let rev = lock_rev(input)?;
            if rev.is_none() {
                eprintln!(
                    "{}",
                    paint(
                        YELLOW,
                        &format!(
                            "upstream-sync: drift: {}: input {input} has no locked rev in \
                             flake.lock",
                            fork.name
                        )
                    )
                );
            }
            rev
        }
        mapping::Source::Vendored(_) => None,
    };
    let forge = if mapping::is_github(&fork.upstream_url) {
        "github"
    } else if mapping::is_gitlab(&fork.upstream_url) {
        "gitlab"
    } else {
        "other"
    };

    let behind = match (forge, rev.as_deref()) {
        ("github", Some(rev)) => github_behind(fork, &slug, rev)?,
        _ => None,
    };
    let base_date = base_date(fork, forge, &slug, rev.as_deref())?;
    let age_days = base_date.as_deref().map(age_days).transpose()?;

    let stances = stance_counts(fork);
    let retired = status::Doc::load(&status::path(fork))?
        .patches
        .values()
        .filter(|p| p.retired)
        .count();

    // Next-action heuristic, deliberately simple:
    //   retired > 0            -> rebase-shrinks-series: a base bump drops the
    //                             retired patches as empty cherries.
    //   drift fully unknown    -> unknown: no basis to recommend anything.
    //   >= 200 commits behind
    //   or base >= 90 days old -> rebase-recommended: in practice this bites
    //                             the manual pins (nix/clippy/mesa); autoUpdate
    //                             forks are cron-freshened before they get here.
    //   else                   -> ok
    let action = if retired > 0 {
        "rebase-shrinks-series"
    } else if behind.is_none() && age_days.is_none() {
        "unknown"
    } else if behind.unwrap_or(0) >= 200 || age_days.unwrap_or(0) >= 90 {
        "rebase-recommended"
    } else {
        "ok"
    };
    let note = match forge {
        "gitlab" => "base-age only (gitlab compare skipped)",
        "other" => "unsupported forge",
        _ => "",
    };

    Ok(Row {
        name: fork.name.clone(),
        forge: forge.to_owned(),
        input: fork.source_label()?,
        rev,
        behind,
        base_date,
        age_days,
        attempt: stances.attempt,
        hold: stances.hold,
        never: stances.never,
        rejected: stances.rejected,
        retired,
        action: action.to_owned(),
        note: note.to_owned(),
    })
}

/// "?" marks an unknown cell (forge unreachable or no locked rev) so a degraded
/// row is visibly degraded, not silently zero. The action column sits before the
/// stance counts so an 80-column pipe still shows the verdict.
fn render_table(rows: &[Row]) -> String {
    const HEADERS: [&str; 11] = [
        "fork",
        "base",
        "behind",
        "age (days)",
        "action",
        "attempt",
        "hold",
        "never",
        "rejected",
        "retired",
        "note",
    ];
    let unknown = || "?".to_owned();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.rev
                    .as_deref()
                    .map_or_else(unknown, |v| v.chars().take(12).collect()),
                r.behind.map_or_else(unknown, |v| v.to_string()),
                r.age_days.map_or_else(unknown, |v| v.to_string()),
                r.action.clone(),
                r.attempt.to_string(),
                r.hold.to_string(),
                r.never.to_string(),
                r.rejected.to_string(),
                r.retired.to_string(),
                r.note.clone(),
            ]
        })
        .collect();
    report::markdown_table(&HEADERS, &cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_stub(
        name: &str,
        rev: Option<&str>,
        behind: Option<i64>,
        age: Option<i64>,
        action: &str,
        note: &str,
    ) -> Row {
        Row {
            name: name.to_owned(),
            forge: "github".to_owned(),
            input: format!("{name}-src"),
            rev: rev.map(str::to_owned),
            behind,
            base_date: None,
            age_days: age,
            attempt: 0,
            hold: 0,
            never: 0,
            rejected: 0,
            retired: 0,
            action: action.to_owned(),
            note: note.to_owned(),
        }
    }

    // Pinned against the nu predecessor's `to md --pretty` bytes: the
    // fork-sync workflow embeds this table in step summaries and PR bodies.
    #[test]
    fn markdown_table_matches_nu_to_md_pretty() {
        let rows = [
            row_stub(
                "fake",
                Some("aaaaaaaaaaaaaaaaaaaa"),
                Some(123),
                Some(194),
                "rebase-recommended",
                "",
            ),
            row_stub("bad", None, None, None, "unknown", "x"),
        ];
        let expected = "\
| fork | base         | behind | age (days) | action             | attempt | hold | never | rejected | retired | note |
| ---- | ------------ | ------ | ---------- | ------------------ | ------- | ---- | ----- | -------- | ------- | ---- |
| fake | aaaaaaaaaaaa | 123    | 194        | rebase-recommended | 0       | 0    | 0     | 0        | 0       |      |
| bad  | ?            | ?      | ?          | unknown            | 0       | 0    | 0     | 0        | 0       | x    |";
        assert_eq!(render_table(&rows), expected);
    }
}
