//! Turn a [`crate::jj`] or [`crate::git`] head into the one line starship
//! prints.
//!
//! The binary colors its own output instead of leaning on the custom module's
//! `style`, because one segment carries several colors (branch, counts,
//! in-progress state) the way starship's own `git_*` modules do.

use std::fmt::Write as _;
use std::time::Duration;

use anstyle::{AnsiColor, Reset, Style};

use crate::git::{self, HeadName};
use crate::jj;
use crate::views;

/// nf-dev-git_branch, the symbol this prompt used for `git_branch`.
const GIT_SYMBOL: &str = "\u{e0a0} ";
/// nf-md-source_commit_start, the closest thing to a jj mark in Nerd Fonts.
const JJ_SYMBOL: &str = "\u{f15c6} ";

const NAME: Style = AnsiColor::Magenta.on_default().bold();
const COUNTS: Style = AnsiColor::Red.on_default().bold();
const MUTED: Style = Style::new().dimmed();

/// A segment under construction: text plus whether escapes are wanted.
struct Segment {
    text: String,
    color: bool,
}

impl Segment {
    const fn new(color: bool) -> Self {
        Self {
            text: String::new(),
            color,
        }
    }

    fn push(&mut self, style: Style, text: &str) {
        if self.color {
            let _ = write!(self.text, "{}{text}{}", style.render(), Reset.render());
        } else {
            self.text.push_str(text);
        }
    }

    fn push_plain(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn into_text(self) -> String {
        self.text
    }
}

/// How stale a view's survey record may be before the segment says so. Under
/// this, the counts are effectively now and the vintage is noise; over it, the
/// vintage is the most important thing about them.
const SURVEY_FRESH: Duration = Duration::from_mins(15);

/// `on 󱗆 ykosps main⇕⇡10⇣97 * ix⇣241·8h`: the working-copy change id, the
/// local bookmark naming it, where it stands against trunk, the state flags,
/// and the view it sits in with the last survey's counts and their vintage.
///
/// Every number names a comparison a reader can restate: `⇡` is `trunk()..@`,
/// `⇣` is `@..trunk()`, and the view's arrows are the last survey against the
/// published repository, dated. There is no separate dirty count: in jj the
/// edits are already in @, so a non-empty working copy is the dirty signal.
pub fn jj(head: &jj::Head, view: Option<&views::View>, color: bool) -> String {
    let mut segment = Segment::new(color);
    segment.push_plain("on ");
    segment.push(NAME, JJ_SYMBOL);
    segment.push(NAME, &head.change_prefix);
    // The dimmed remainder of the 8-char change id carried no information the
    // prefix does not; commented out to keep the segment short.
    // segment.push(MUTED, &head.change_rest);

    // The bookmark names the change; the trunk comparison places it. When the
    // nearest bookmark *is* trunk the name is printed once, with the arrows.
    let trunk_name = head.trunk.as_ref().map(|trunk| trunk.name.as_str());
    if let Some(bookmark) = &head.bookmark
        && Some(bookmark.as_str()) != trunk_name
    {
        segment.push_plain(" ");
        segment.push(NAME, bookmark);
    }
    if let Some(trunk) = &head.trunk {
        segment.push_plain(" ");
        segment.push(NAME, &trunk.name);
        let arrows = ahead_behind(trunk.ahead, trunk.behind);
        if !arrows.is_empty() {
            segment.push(COUNTS, &arrows);
        }
    }

    let mut flags = String::new();
    if head.flags.conflict {
        flags.push('=');
    }
    if head.flags.divergent {
        flags.push_str("??");
    }
    if !head.flags.empty {
        flags.push('*');
    }
    if !flags.is_empty() {
        segment.push_plain(" ");
        segment.push(COUNTS, &flags);
    }

    // The view the directory is inside: context the way the submodule
    // breadcrumb is, so the name is muted, with the last survey's counts
    // against the published repository beside it.
    if let Some(view) = view {
        segment.push_plain(" ");
        segment.push(MUTED, &view.name);
        if let Some(counts) = view.counts {
            let arrows = ahead_behind(counts.ahead, counts.behind);
            if !arrows.is_empty() {
                segment.push(COUNTS, &arrows);
                // The vintage rides with the counts or not at all: an arrow
                // with no date reads as "now", which is the misreading this
                // suffix exists to prevent.
                if let Some(vintage) = view.age.and_then(vintage) {
                    segment.push(MUTED, &vintage);
                }
            }
        }
    }

    segment.into_text()
}

/// `on  main !2?1⇡2`, matching the symbols the disabled `git_branch` and
/// `git_status` modules were configured with.
pub fn git(head: &git::Head, color: bool) -> String {
    let mut segment = Segment::new(color);
    segment.push_plain("on ");
    segment.push(NAME, GIT_SYMBOL);
    match &head.name {
        HeadName::Branch(branch) => segment.push(NAME, branch),
        HeadName::Detached(commit) => segment.push(NAME, &format!("({commit})")),
    }

    let counts = counts(&head.counts, head.tracking);
    if !counts.is_empty() {
        segment.push_plain(" ");
        segment.push(COUNTS, &counts);
    }

    segment.into_text()
}

/// Status counts in starship's `$all_status$ahead_behind` order and symbols,
/// so the segment reads the same as before the modules moved in here.
fn counts(counts: &git::Counts, tracking: Option<git::Tracking>) -> String {
    let mut rendered = String::new();
    for (symbol, count) in [
        ("=", counts.conflicted),
        ("✘", counts.deleted),
        ("»", counts.renamed),
        ("!", counts.modified),
        ("+", counts.staged),
        ("?", counts.untracked),
    ] {
        if count > 0 {
            let _ = write!(rendered, "{symbol}{count}");
        }
    }

    if let Some(git::Tracking { ahead, behind }) = tracking {
        rendered.push_str(&ahead_behind(ahead, behind));
    }

    rendered
}

/// A survey record's age as one dense token, `·8h`, or `None` while the
/// record is fresh enough that its counts can be read as current.
fn vintage(age: Duration) -> Option<String> {
    if age < SURVEY_FRESH {
        return None;
    }
    let minutes = age.as_secs() / 60;
    let (value, unit) = match minutes {
        ..60 => (minutes, 'm'),
        60..1440 => (minutes / 60, 'h'),
        _ => (minutes / 1440, 'd'),
    };
    Some(format!("\u{b7}{value}{unit}"))
}

/// The ahead/behind arrows, shared by the git tracking counts and the jj
/// view counts so the two read identically.
fn ahead_behind(ahead: usize, behind: usize) -> String {
    match (ahead, behind) {
        (0, 0) => String::new(),
        (ahead, 0) => format!("⇡{ahead}"),
        (0, behind) => format!("⇣{behind}"),
        (ahead, behind) => format!("⇕⇡{ahead}⇣{behind}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::git::{Counts as GitCounts, Head as GitHead, HeadName, Tracking};
    use crate::jj::{Flags, Head as JjHead, Trunk};
    use crate::views::{Counts, View};

    fn head(bookmark: Option<&str>, trunk: Option<Trunk>, empty: bool) -> JjHead {
        JjHead {
            change_prefix: "ykosps".to_owned(),
            change_rest: "vq".to_owned(),
            flags: Flags {
                empty,
                conflict: false,
                divergent: false,
            },
            bookmark: bookmark.map(str::to_owned),
            trunk,
        }
    }

    fn trunk(name: &str, ahead: usize, behind: usize) -> Trunk {
        Trunk {
            name: name.to_owned(),
            ahead,
            behind,
        }
    }

    fn view(name: &str, counts: Option<Counts>, age: Option<Duration>) -> View {
        View {
            name: name.to_owned(),
            counts,
            age,
        }
    }

    /// The regression this rewrite exists for. The old segment rendered
    /// `main@git+10` -- a distance to jj's `git` pseudo-remote -- and hid the
    /// 97 commits of trunk that @ did not have.
    #[test]
    fn a_diverged_working_copy_shows_both_sides_against_a_named_trunk() {
        let rendered = super::jj(&head(None, Some(trunk("main*", 10, 97)), false), None, false);

        assert_eq!(rendered, "on \u{f15c6} ykosps main*\u{21d5}\u{21e1}10\u{21e3}97 *");
        assert!(!rendered.contains("@git"), "a pseudo-remote reached the prompt");
    }

    #[test]
    fn a_working_copy_level_with_trunk_shows_the_name_alone() {
        assert_eq!(
            super::jj(&head(None, Some(trunk("main", 0, 0)), true), None, false),
            "on \u{f15c6} ykosps main"
        );
    }

    #[test]
    fn only_ahead_of_trunk_is_one_arrow() {
        assert_eq!(
            super::jj(&head(None, Some(trunk("ix-patched", 2, 0)), false), None, false),
            "on \u{f15c6} ykosps ix-patched\u{21e1}2 *"
        );
    }

    /// A bookmark that is not trunk is worth its own name; a bookmark that
    /// *is* trunk must not be printed twice.
    #[test]
    fn a_bookmark_off_trunk_is_named_beside_it() {
        assert_eq!(
            super::jj(
                &head(Some("feature"), Some(trunk("main", 3, 1)), true),
                None,
                false
            ),
            "on \u{f15c6} ykosps feature main\u{21d5}\u{21e1}3\u{21e3}1"
        );
    }

    #[test]
    fn a_bookmark_that_is_trunk_is_printed_once() {
        assert_eq!(
            super::jj(
                &head(Some("main*"), Some(trunk("main*", 1, 0)), true),
                None,
                false
            ),
            "on \u{f15c6} ykosps main*\u{21e1}1"
        );
    }

    /// With no trunk to compare against, the segment says less rather than
    /// inventing a comparison.
    #[test]
    fn no_trunk_leaves_the_change_id_and_flags() {
        assert_eq!(
            super::jj(&head(None, None, false), None, false),
            "on \u{f15c6} ykosps *"
        );
    }

    #[test]
    fn a_fresh_survey_needs_no_vintage() {
        let counts = Some(Counts {
            behind: 25,
            ahead: 0,
        });
        assert_eq!(
            super::jj(
                &head(None, None, true),
                Some(&view("ix", counts, Some(Duration::from_mins(2)))),
                false
            ),
            "on \u{f15c6} ykosps ix\u{21e3}25"
        );
    }

    /// The bug this suffix exists for: a count with no date reads as "now".
    #[test]
    fn a_stale_survey_carries_its_vintage() {
        let counts = Some(Counts {
            behind: 241,
            ahead: 1,
        });
        let age = Some(Duration::from_mins(7 * 60 + 41));
        assert_eq!(
            super::jj(
                &head(None, None, true),
                Some(&view("ix", counts, age)),
                false
            ),
            "on \u{f15c6} ykosps ix\u{21d5}\u{21e1}1\u{21e3}241\u{b7}7h"
        );
    }

    #[test]
    fn a_view_with_nothing_to_report_is_just_its_name() {
        for counts in [
            None,
            Some(Counts {
                behind: 0,
                ahead: 0,
            }),
        ] {
            assert_eq!(
                super::jj(
                    &head(None, None, true),
                    Some(&view("ix", counts, Some(Duration::from_hours(27)))),
                    false
                ),
                "on \u{f15c6} ykosps ix",
                "a zero count must not drag a vintage in behind it"
            );
        }
    }

    #[test]
    fn vintage_units_step_from_minutes_through_days() {
        assert_eq!(super::vintage(Duration::from_mins(1)), None);
        assert_eq!(super::vintage(Duration::from_mins(20)), Some("\u{b7}20m".to_owned()));
        assert_eq!(super::vintage(Duration::from_hours(3)), Some("\u{b7}3h".to_owned()));
        assert_eq!(
            super::vintage(Duration::from_hours(73)),
            Some("\u{b7}3d".to_owned())
        );
    }

    #[test]
    fn conflict_and_divergence_still_reach_the_flags() {
        let mut h = head(None, None, false);
        h.flags.conflict = true;
        h.flags.divergent = true;
        assert_eq!(super::jj(&h, None, false), "on \u{f15c6} ykosps =??*");
    }

    #[test]
    fn git_counts_follow_starship_order_and_symbols() {
        let head = GitHead {
            name: HeadName::Branch("main".to_owned()),
            tracking: Some(Tracking {
                ahead: 2,
                behind: 1,
            }),
            counts: GitCounts {
                modified: 3,
                untracked: 1,
                ..GitCounts::default()
            },
        };

        assert_eq!(super::git(&head, false), "on \u{e0a0} main !3?1\u{21d5}\u{21e1}2\u{21e3}1");
    }

    #[test]
    fn a_detached_head_shows_the_commit_in_parentheses() {
        let head = GitHead {
            name: HeadName::Detached("c1b4a88".to_owned()),
            tracking: None,
            counts: GitCounts::default(),
        };

        assert_eq!(super::git(&head, false), "on \u{e0a0} (c1b4a88)");
    }
}
