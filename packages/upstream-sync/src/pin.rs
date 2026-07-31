//! Pin drift against our own fork bookmark (ENG-11630).
//!
//! [`drift`](crate::drift) answers how far a pinned base trails UPSTREAM. This
//! answers the question nothing was asking: is the pinned rev still ON the
//! fork's own bookmark, and how far behind its tip.
//!
//! Only this direction catches a pin nobody moved, and it catches it silently
//! late. A rev-pinned fork whose megamerge gets rebased keeps building the old
//! rev forever, because the fork push mints a permanent
//! `refs/pins/<date>-<sha12>` ref in the same operation: the orphaned commit
//! stays fetchable, eval stays green, and the only symptom is that the tree
//! that ships is not the tree on the branch, which is the tree anyone
//! reviewing the fork reads.
//!
//! That is the incident this exists for. On 2026-07-31 the daemon toolchain pin
//! sat at `0f356d7cf513` while `indexable-inc/nix` `ix-patched` was at
//! `f200a3a8d492`: `compare/0f356d7cf513...ix-patched` answered `diverged`, 72
//! ahead / 54 behind, merge base `2c6d06e9387c`. Sixteen patches, the whole jj
//! fetcher series among them, were on the branch and absent from every build,
//! and no check in this repo was red, because nothing compared the two revs.
//!
//! The classification comes out of one `compare/<pinned>...<bookmark>` read,
//! pinned rev as base and bookmark as head:
//!
//! - `identical` -> current.
//! - `ahead` (the bookmark is a descendant) -> behind by N patches.
//!   Informational: the pin is still on the branch, it is just not the tip.
//! - `behind` or `diverged` -> diverged. The pinned rev is not reachable from
//!   the bookmark, so no rebase or repin can be described as "moving forward".
//!
//! A diverged pin fails for a rev-pinned fork and not for a floating one, and
//! the asymmetry is real rather than a concession. `autoUpdate = true` forks
//! are rebased and re-locked by the same fork-sync run, and a jj rebase
//! REWRITES the series, so between that rebase and its rolling PR merging,
//! main's lock legitimately points off the branch. An `autoUpdate = false`
//! fork has no such lane: its pin moves when a human moves it, which is to say
//! it stays wherever it was left.

use std::path::Path;

use anstream::{eprintln, println};
use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::{Serialize, Serializer};

use crate::drift::lock_rev;
use crate::gh;
use crate::mapping::{self, Fork};
use crate::report;
use crate::style::{GREEN, RED, paint};

/// Where a pinned rev sits relative to its fork bookmark.
///
/// The word IS the value rather than a match arm away from it. It renders in the
/// table, in `--json` and in the failure message, and one of those going out of
/// step with the others is the exact class of drift this tool exists to catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Class(&'static str);

impl Class {
    /// The pin IS the bookmark tip.
    pub const CURRENT: Self = Self("current");
    /// The bookmark is a descendant of the pin: published, unpinned patches.
    pub const BEHIND: Self = Self("behind");
    /// The pin is not an ancestor of the bookmark at all.
    pub const DIVERGED: Self = Self("diverged");
    /// No answer: the input is absent from flake.lock, or the forge said the
    /// bookmark or the rev is not there.
    pub const UNKNOWN: Self = Self("unknown");

    const fn word(self) -> &'static str {
        self.0
    }
}

impl Serialize for Class {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

/// One fork's pin standing (the `--json` row shape).
#[derive(Debug, Serialize)]
pub struct Row {
    pub name: String,
    pub input: String,
    #[serde(rename = "forkRepo")]
    pub fork_repo: String,
    pub bookmark: String,
    /// What flake.lock pins, or `None` when the input is not in the lock.
    pub rev: Option<String>,
    /// The bookmark tip, or `None` when the forge would not name it.
    pub tip: Option<String>,
    pub class: Class,
    /// Commits the bookmark has that the pin does not.
    pub behind: Option<i64>,
    /// Commits the pinned tree has that the bookmark does not. Non-zero only
    /// when diverged, and the reason a diverged pin cannot be fixed by a bump.
    #[serde(rename = "aheadOfBookmark")]
    pub ahead: Option<i64>,
    #[serde(rename = "mergeBase")]
    pub merge_base: Option<String>,
    /// Whether a divergence in this row fails the gate.
    pub gated: bool,
    /// Why this row is informational, when it is.
    pub note: String,
    /// The gate's complaint about this row, if it has one.
    pub problem: Option<String>,
}

/// One `compare/<base>...<head>` answer. The four fields travel together
/// because they are one answer: a status word with no counts is not reportable,
/// and counts with no merge base are not actionable.
struct Compare {
    status: String,
    ahead_by: i64,
    behind_by: i64,
    merge_base: String,
}

/// Read the pinned rev against the bookmark tip in one forge call.
///
/// `None` means the forge gave a definitive "not here" (see [`gh::read`]), which
/// becomes an unknown row rather than a failure.
fn compare(name: &str, fork_repo: &str, base: &str, head: &str) -> Result<Option<Compare>> {
    let path = format!("repos/{fork_repo}/compare/{base}...{head}");
    let gh::Read::Value(raw) = gh::read(
        &format!("pin-drift: {name}"),
        &path,
        "[.status, .ahead_by, .behind_by, .merge_base_commit.sha] | @tsv",
    )?
    else {
        return Ok(None);
    };
    let fields: Vec<&str> = raw.split('\t').collect();
    let [status, ahead, behind, merge_base] = fields.as_slice() else {
        return Err(eyre!(
            "upstream-sync: pin-drift: {name}: {path} answered {} tab-separated field(s), want 4: {raw:?}",
            fields.len()
        ));
    };
    let parse = |what: &str, v: &str| -> Result<i64> {
        v.parse::<i64>().wrap_err_with(|| {
            format!("upstream-sync: pin-drift: {name}: {path} answered a non-numeric {what}: {v:?}")
        })
    };
    Ok(Some(Compare {
        status: (*status).to_owned(),
        ahead_by: parse("ahead_by", ahead)?,
        behind_by: parse("behind_by", behind)?,
        merge_base: (*merge_base).to_owned(),
    }))
}

/// The forge's compare status word is the whole classification.
///
/// `behind` folds into [`Class::Diverged`] wearing a friendlier word: it says
/// the bookmark is an ancestor of the pin, so the pinned rev is not reachable
/// from the branch, which is the same defect. An unrecognised word is an error
/// and not a default, because every default available here reads as "no drift".
fn classify(status: &str) -> Result<Class> {
    match status {
        "identical" => Ok(Class::CURRENT),
        "ahead" => Ok(Class::BEHIND),
        "behind" | "diverged" => Ok(Class::DIVERGED),
        other => Err(eyre!(
            "upstream-sync: pin-drift: the forge answered compare status {other:?}, which this \
             tool cannot classify. Refusing to guess: every guess available here reads as \"no \
             drift\"."
        )),
    }
}

/// Whether this row fails, and what to say about it either way.
struct Verdict {
    gated: bool,
    note: String,
    problem: Option<String>,
}

/// The waiver is keyed by the rev it covers, so it expires the moment the pin
/// moves and cannot be inherited by a pin nobody looked at.
fn verdict(
    fork: &Fork,
    class: Class,
    rev: Option<&str>,
    tip: Option<&str>,
    cmp: Option<&Compare>,
) -> Verdict {
    let waiver = fork.pin_divergence.as_ref();
    let covers = waiver.is_some_and(|w| rev.is_some_and(|r| r == w.rev));

    if class != Class::DIVERGED {
        // A waiver over a pin that is not diverged waives nothing, and dead
        // intent is worse than no intent: the next person reads it as a live
        // exemption and stops asking.
        let problem = waiver.map(|_| {
            format!(
                "{}: pinDivergence is declared but the pin is {}. The waiver covers nothing, so \
                 delete it rather than leave a live-looking exemption behind.",
                fork.name,
                class.word()
            )
        });
        return Verdict {
            gated: !fork.auto_update && waiver.is_none(),
            note: String::new(),
            problem,
        };
    }

    if fork.auto_update {
        return Verdict {
            gated: false,
            note: "floating input: fork-sync rebases and re-locks together, so main is off the \
                   branch until the rolling PR merges"
                .to_owned(),
            problem: None,
        };
    }
    if covers {
        let reason = waiver.map_or("", |w| w.reason.as_str());
        return Verdict {
            gated: false,
            note: format!("waived: {reason}"),
            problem: None,
        };
    }

    let mut lines = vec![format!(
        "{}: the pinned rev is not an ancestor of {} {}.",
        fork.name, fork.fork_repo, fork.bookmark
    )];
    lines.push(format!(
        "  pinned     {} (flake.lock {})",
        rev.unwrap_or("<absent>"),
        fork.input
    ));
    lines.push(format!(
        "  bookmark   {} ({})",
        tip.unwrap_or("<absent>"),
        fork.bookmark
    ));
    if let Some(cmp) = cmp {
        lines.push(format!("  merge base {}", cmp.merge_base));
        lines.push(format!(
            "  {} commit(s) on the bookmark are unpinned, and {} commit(s) in the pinned tree are \
             not on the bookmark, which is why no bump fixes this by itself.",
            cmp.ahead_by, cmp.behind_by
        ));
    }
    lines.push(
        "  Repin to the bookmark tip, or push the bookmark to what is pinned. Whichever is right, \
         the two have to be one tree; a permanent refs/pins ref keeps the orphan building either \
         way, so nothing else will tell you."
            .to_owned(),
    );
    if let Some(w) = waiver {
        lines.push(format!(
            "  pinDivergence names {}, which is no longer the pinned rev, so the waiver has \
             expired. Its reason was: {}",
            w.rev, w.reason
        ));
    }
    Verdict {
        gated: true,
        note: String::new(),
        problem: Some(lines.join("\n")),
    }
}

fn row(fork: &Fork) -> Result<Row> {
    let ctx = format!("pin-drift: {}", fork.name);
    let rev = lock_rev(&fork.input)?;
    let tip = match gh::read(
        &ctx,
        &format!("repos/{}/commits/{}", fork.fork_repo, fork.bookmark),
        ".sha",
    )? {
        gh::Read::Value(sha) => Some(sha),
        gh::Read::Absent(_) => None,
    };

    let cmp = match (rev.as_deref(), tip.as_deref()) {
        (Some(rev), Some(tip)) => compare(&fork.name, &fork.fork_repo, rev, tip)?,
        _ => None,
    };
    let class = match &cmp {
        Some(cmp) => classify(&cmp.status)?,
        None => Class::UNKNOWN,
    };
    let v = verdict(fork, class, rev.as_deref(), tip.as_deref(), cmp.as_ref());
    let note = if class == Class::UNKNOWN && v.note.is_empty() {
        unknown_note(rev.as_deref(), tip.as_deref())
    } else {
        v.note
    };

    Ok(Row {
        name: fork.name.clone(),
        input: fork.input.clone(),
        fork_repo: fork.fork_repo.clone(),
        bookmark: fork.bookmark.clone(),
        rev,
        tip,
        class,
        behind: cmp.as_ref().map(|c| c.ahead_by),
        ahead: cmp.as_ref().map(|c| c.behind_by),
        merge_base: cmp.map(|c| c.merge_base),
        gated: v.gated,
        note,
        problem: v.problem,
    })
}

fn unknown_note(rev: Option<&str>, tip: Option<&str>) -> String {
    match (rev, tip) {
        (None, _) => "no locked rev for this input in flake.lock".to_owned(),
        (_, None) => "the forge does not have this bookmark".to_owned(),
        _ => "the forge would not compare the pin with the bookmark".to_owned(),
    }
}

/// Report every fork's pin against its bookmark, and FAIL on a divergence that
/// nothing else will report.
///
/// # Errors
/// Fails on flag conflicts, an unknown fork name, unreadable local data, a
/// forge that cannot be reached, and on any gated divergence or stale waiver.
pub fn run(
    mapping_override: Option<&Path>,
    name: Option<&str>,
    json: bool,
    markdown: bool,
) -> Result<()> {
    report::check_flags("pin-drift", json, markdown)?;
    let mapping_path = mapping::path(mapping_override)?;
    let forks = mapping::select(mapping::load(&mapping_path)?, name, "upstream-sync")?;
    let rows = forks.iter().map(row).collect::<Result<Vec<Row>>>()?;
    report::emit(
        &rows,
        &render_table(&rows),
        "== fork pins: pinned rev vs the fork's own bookmark ==",
        json,
        markdown,
    )?;

    let problems: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.problem.as_deref())
        .collect();
    if problems.is_empty() {
        if !json && !markdown {
            println!("{}", paint(GREEN, "every fork pin is on its bookmark"));
        }
        return Ok(());
    }
    eprintln!("{}", paint(RED, &problems.join("\n\n")));
    Err(eyre!(
        "upstream-sync: pin-drift: {} fork pin(s) are off their bookmark",
        problems.len()
    ))
}

/// "?" marks a cell the forge would not fill, so a degraded row is visibly
/// degraded rather than silently zero. A row the gate fails is marked in the
/// state column, because the table and the failure detail are read separately:
/// the table lands in a step summary, the detail in the job log.
fn render_table(rows: &[Row]) -> String {
    const HEADERS: [&str; 8] = [
        "fork",
        "pin",
        "bookmark",
        "tip",
        "state",
        "behind",
        "ahead",
        "note",
    ];
    let unknown = || "?".to_owned();
    let short = |sha: Option<&str>| {
        sha.map_or_else(unknown, |v| v.chars().take(12).collect::<String>())
    };
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                short(r.rev.as_deref()),
                r.bookmark.clone(),
                short(r.tip.as_deref()),
                if r.problem.is_some() {
                    format!("{} (FAIL)", r.class.word())
                } else {
                    r.class.word().to_owned()
                },
                r.behind.map_or_else(unknown, |v| v.to_string()),
                r.ahead.map_or_else(unknown, |v| v.to_string()),
                r.note.clone(),
            ]
        })
        .collect();
    report::markdown_table(&HEADERS, &cells)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::PinDivergence;

    fn fork(name: &str, auto_update: bool, waiver: Option<PinDivergence>) -> Fork {
        Fork {
            name: name.to_owned(),
            input: format!("{name}-src"),
            fork_repo: format!("indexable-inc/{name}"),
            bookmark: "ix-patched".to_owned(),
            upstream_url: format!("https://github.com/upstream/{name}.git"),
            upstream_ref: None,
            auto_update,
            pin_divergence: waiver,
            patches: std::collections::BTreeMap::new(),
            upstream_policy: None,
        }
    }

    fn cmp() -> Compare {
        Compare {
            status: "diverged".to_owned(),
            ahead_by: 72,
            behind_by: 54,
            merge_base: "2c6d06e9387cf58167cb5a7ab91cee7333d8d17c".to_owned(),
        }
    }

    #[test]
    fn compare_status_words_map_to_classes() {
        assert_eq!(classify("identical").unwrap(), Class::CURRENT);
        assert_eq!(classify("ahead").unwrap(), Class::BEHIND);
        assert_eq!(classify("diverged").unwrap(), Class::DIVERGED);
        // The bookmark trailing the pin is the same defect, so it must not
        // land anywhere softer than diverged.
        assert_eq!(classify("behind").unwrap(), Class::DIVERGED);
    }

    // An unrecognised status must not become a class at all: the only wrong
    // answers available are "current" and "behind", and both read as no drift.
    #[test]
    fn an_unknown_status_word_is_an_error() {
        let err = classify("sideways").unwrap_err().to_string();
        assert!(err.contains("sideways"), "{err}");
        assert!(err.contains("Refusing to guess"), "{err}");
    }

    #[test]
    fn a_rev_pinned_divergence_fails_naming_both_shas_and_the_merge_base() {
        let f = fork("nix", false, None);
        let v = verdict(
            &f,
            Class::DIVERGED,
            Some("0f356d7cf513ca074a2122079defeb95810b6a91"),
            Some("f200a3a8d4921393547f93166cce8cebcb2b0e44"),
            Some(&cmp()),
        );
        assert!(v.gated);
        let problem = v.problem.expect("a gated divergence states its problem");
        assert!(problem.contains("0f356d7cf513ca074a2122079defeb95810b6a91"), "{problem}");
        assert!(problem.contains("f200a3a8d4921393547f93166cce8cebcb2b0e44"), "{problem}");
        assert!(problem.contains("2c6d06e9387cf58167cb5a7ab91cee7333d8d17c"), "{problem}");
        assert!(problem.contains("72 commit(s)"), "{problem}");
        assert!(problem.contains("54 commit(s)"), "{problem}");
    }

    #[test]
    fn a_floating_input_is_reported_and_not_gated() {
        let f = fork("btop", true, None);
        let v = verdict(&f, Class::DIVERGED, Some("9f43b7904e2d"), None, Some(&cmp()));
        assert!(!v.gated);
        assert!(v.problem.is_none());
        assert!(v.note.contains("rolling PR"), "{}", v.note);
    }

    #[test]
    fn a_waiver_covers_only_the_rev_it_names() {
        let waiver = PinDivergence {
            rev: "69fbc5cfd883".to_owned(),
            reason: "ENG-11646".to_owned(),
        };
        let f = fork("git", false, Some(waiver));
        let covered = verdict(&f, Class::DIVERGED, Some("69fbc5cfd883"), None, Some(&cmp()));
        assert!(!covered.gated);
        assert!(covered.note.contains("ENG-11646"), "{}", covered.note);

        // The pin moved, so the acknowledgement someone made about the old rev
        // says nothing about this one.
        let moved = verdict(&f, Class::DIVERGED, Some("aaaaaaaaaaaa"), None, Some(&cmp()));
        assert!(moved.gated);
        let problem = moved.problem.expect("an expired waiver still fails");
        assert!(problem.contains("expired"), "{problem}");
        assert!(problem.contains("69fbc5cfd883"), "{problem}");
    }

    #[test]
    fn a_waiver_over_a_healthy_pin_is_itself_the_problem() {
        let waiver = PinDivergence {
            rev: "abc123abc123".to_owned(),
            reason: "stale".to_owned(),
        };
        let f = fork("jj", false, Some(waiver));
        let v = verdict(&f, Class::CURRENT, Some("abc123abc123"), None, None);
        let problem = v.problem.expect("a dead waiver is a problem");
        assert!(problem.contains("delete it"), "{problem}");
    }

    // Pinned bytes, for the same reason drift's table is: the fork-sync
    // workflow embeds it in step summaries.
    #[test]
    fn markdown_table_matches_nu_to_md_pretty() {
        let rows = [
            Row {
                name: "nix".to_owned(),
                input: "nix-src".to_owned(),
                fork_repo: "indexable-inc/nix".to_owned(),
                bookmark: "ix-patched".to_owned(),
                rev: Some("0f356d7cf513ca074a2122079defeb95810b6a91".to_owned()),
                tip: Some("f200a3a8d4921393547f93166cce8cebcb2b0e44".to_owned()),
                class: Class::DIVERGED,
                behind: Some(72),
                ahead: Some(54),
                merge_base: Some("2c6d06e9387c".to_owned()),
                gated: true,
                note: String::new(),
                problem: Some("nix: ...".to_owned()),
            },
            Row {
                name: "bad".to_owned(),
                input: "bad-src".to_owned(),
                fork_repo: "indexable-inc/bad".to_owned(),
                bookmark: "ix-patched".to_owned(),
                rev: None,
                tip: None,
                class: Class::UNKNOWN,
                behind: None,
                ahead: None,
                merge_base: None,
                gated: false,
                note: "no locked rev".to_owned(),
                problem: None,
            },
        ];
        let expected = "\
| fork | pin          | bookmark   | tip          | state           | behind | ahead | note          |
| ---- | ------------ | ---------- | ------------ | --------------- | ------ | ----- | ------------- |
| nix  | 0f356d7cf513 | ix-patched | f200a3a8d492 | diverged (FAIL) | 72     | 54    |               |
| bad  | ?            | ix-patched | ?            | unknown         | ?      | ?     | no locked rev |";
        assert_eq!(render_table(&rows), expected);
    }
}
