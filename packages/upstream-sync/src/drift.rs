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

use anstream::{eprintln, println};
use chrono::{DateTime, Utc};
use color_eyre::eyre::{Result, WrapErr, eyre};
use lazy_regex::regex;
use serde::Serialize;

use crate::gh;
use crate::mapping::{self, Fork, Slug};
use crate::status;
use crate::style::{CYAN, YELLOW, paint};

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
    if json && markdown {
        return Err(eyre!(
            "upstream-sync: drift: --json and --markdown are mutually exclusive"
        ));
    }
    let mapping_path = mapping::path(mapping_override)?;
    let forks = mapping::select(mapping::load(&mapping_path)?, name, "upstream-sync")?;
    let rows = forks.iter().map(row).collect::<Result<Vec<Row>>>()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let table = render_table(&rows);
    if markdown {
        println!("{table}");
    } else {
        println!(
            "{}",
            paint(
                CYAN,
                "== fork drift: pinned base vs upstream default branch =="
            )
        );
        println!("{table}");
    }
    Ok(())
}

/// The pinned base rev of a fork's input from the committed flake.lock in
/// the CWD (the tool runs from the repo root; a downstream --mapping repo
/// reads its own lock the same way). `None` when the lock or input is
/// absent.
///
/// # Errors
/// Fails when an existing flake.lock is unreadable or not JSON.
fn lock_rev(input: &str) -> Result<Option<String>> {
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
    let repo = format!("repos/{}/{}", slug.owner, slug.repo);
    let branch = match &fork.upstream_ref {
        Some(configured) => configured.clone(),
        None => match gh::read(&fork.name, &repo, ".default_branch")? {
            Some(branch) => branch,
            None => return Ok(None),
        },
    };
    let Some(n) = gh::read(
        &fork.name,
        &format!("{repo}/compare/{rev}...{branch}"),
        ".ahead_by",
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
fn gitlab_base_date(url: &str, rev: &str) -> Option<String> {
    let caps = regex!(r"^https?://([^/]+)/(.+)$").captures(url)?;
    let host = &caps[1];
    let path = caps[2].trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    // Percent-encode every byte (the API wants the project path as ONE
    // segment; encoding alphanumerics too is legal and matches `url encode
    // --all` in the nu predecessor).
    let project = path.bytes().fold(String::new(), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "%{b:02X}");
        acc
    });
    let endpoint = format!("https://{host}/api/v4/projects/{project}/repository/commits/{rev}");

    let warn = || {
        eprintln!(
            "{}",
            paint(
                YELLOW,
                &format!("upstream-sync: drift: {endpoint} unreachable; base age left unknown")
            )
        );
    };
    let Ok(mut res) = ureq::get(&endpoint).call() else {
        warn();
        return None;
    };
    let Ok(v) = res.body_mut().read_json::<serde_json::Value>() else {
        warn();
        return None;
    };
    let date = v.get("committed_date").and_then(serde_json::Value::as_str);
    if date.is_none() {
        warn();
    }
    date.map(str::to_owned)
}

fn base_date(fork: &Fork, forge: &str, slug: &Slug, rev: Option<&str>) -> Result<Option<String>> {
    let Some(rev) = rev else { return Ok(None) };
    match forge {
        "github" => gh::read(
            &fork.name,
            &format!("repos/{}/{}/commits/{rev}", slug.owner, slug.repo),
            ".commit.committer.date",
        ),
        "gitlab" => Ok(gitlab_base_date(&fork.upstream_url, rev)),
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
    let rev = lock_rev(&fork.input)?;
    if rev.is_none() {
        eprintln!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: drift: {}: input {} has no locked rev in flake.lock",
                    fork.name, fork.input
                )
            )
        );
    }
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
        input: fork.input.clone(),
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

/// The human/markdown table: "?" marks an unknown cell (forge unreachable or
/// no locked rev) so a degraded row is visibly degraded, not silently zero.
/// Byte-compatible with nu's `to md --pretty` (the fork-sync workflow embeds
/// it in step summaries and PR bodies). The action column sits before the
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
    let cells: Vec<[String; 11]> = rows
        .iter()
        .map(|r| {
            [
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

    let widths: Vec<usize> = HEADERS
        .iter()
        .enumerate()
        .map(|(i, h)| {
            cells
                .iter()
                .map(|row| row[i].len())
                .chain([h.len()])
                .max()
                .unwrap_or_default()
        })
        .collect();
    let line = |vals: &[String]| {
        let padded: Vec<String> = vals
            .iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:<w$}"))
            .collect();
        format!("| {} |", padded.join(" | "))
    };

    let mut out = vec![
        line(&HEADERS.map(str::to_owned)),
        line(
            &widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<String>>(),
        ),
    ];
    out.extend(cells.iter().map(|row| line(row.as_slice())));
    out.join("\n")
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
