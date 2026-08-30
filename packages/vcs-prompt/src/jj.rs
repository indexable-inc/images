//! Working-copy state of a jj workspace, read with one `jj log` call.
//!
//! `jj-lib` would answer the same questions in-process, but it is a heavy
//! dependency to build and pin (it is why the third-party `jj-starship` was
//! dropped from this config), and the CLI answers in ~100ms on the largest
//! workspace here, which a prompt can pay.
//!
//! One call answers everything the prompt shows, including the parent's
//! timestamp for the commit-age segment: the extra revsets are free next to
//! the commit-index load that dominates, measured at 100ms either way on the
//! 4000-commit workspace here.

use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr, eyre};

/// How far back to look for a bookmark to name the working copy after. jj
/// bookmarks do not move on commit, so the useful answer is almost always a
/// few commits back; the cap keeps `jj log` from walking a long unbookmarked
/// history and keeps the parse bounded.
const BOOKMARK_SEARCH_DEPTH: usize = 20;

/// Rows are tagged by set membership rather than by position, because
/// position is not distance: `jj log` emits a topological order, so the index
/// of a bookmarked row in `ancestors(@, N)` is not the number of commits
/// between it and `@` as soon as a merge is in the window. `contained_in`
/// asks the revset engine the question directly, and the counts it yields are
/// the ones a reader can restate: `⇡` is `trunk()..@` less an empty `@`,
/// `⇣` is `@..trunk()`.
///
/// An empty working-copy commit is excluded from `⇡`: jj keeps one on top of
/// whatever was last checked out, and right after `jj bookmark set main -r @`
/// it is the only member of `trunk()..@`. It holds nothing trunk lacks, so
/// counting it rendered a clean checkout of `main` as `main⇡1`. A non-empty
/// `@` does carry edits trunk lacks and still counts (and renders `*`).
///
/// `local_bookmarks`, never `bookmarks`: the latter also yields *remote*
/// bookmarks, and in a non-colocated repo that includes jj's `git`
/// pseudo-remote -- `refs/heads/*` inside `.jj/repo/store/git`, an export
/// mirror that a `jj git push` left behind and that nothing moves afterwards.
/// It out-ran the real trunk here and rendered `main@git+10` while `@` was in
/// fact 10 ahead and 97 behind the actual `main`, naming a comparison against
/// a ref no remote has ever seen.
fn revset() -> String {
    format!("@ | trunk() | trunk()..@ | @..trunk() | ancestors(@, {BOOKMARK_SEARCH_DEPTH})")
}

/// One tab-separated row per commit: the membership tags, the change-id
/// prefix, the rest of the shortest unique change id, the commit's local and
/// remote bookmarks, a flag blob, and the committer epoch. Fields are
/// concatenated rather than passed through the template's `separate()`, which
/// drops empty values and would shift the columns.
fn template() -> String {
    let tags = [
        ("@", "W"),
        ("trunk()", "T"),
        ("trunk()..@", "A"),
        ("@..trunk()", "B"),
        ("root()", "R"),
    ]
    .iter()
    .map(|(revset, tag)| format!(r#"if(self.contained_in("{revset}"),"{tag}","")"#))
    .collect::<Vec<_>>()
    .join(" ++ ");

    format!(
        concat!(
            r#"{tags} ++ "\t""#,
            r#" ++ change_id.shortest(8).prefix() ++ "\t""#,
            r#" ++ change_id.shortest(8).rest() ++ "\t""#,
            r#" ++ local_bookmarks.join(",") ++ "\t""#,
            r#" ++ remote_bookmarks.join(",") ++ "\t""#,
            r#" ++ if(empty, "e", "") ++ if(conflict, "c", "") ++ if(divergent, "d", "") ++ "\n""#,
        ),
        tags = tags
    )
}

/// Where `@` stands against the repository's trunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trunk {
    /// The trunk bookmark as jj renders it, so a local bookmark out of sync
    /// with its tracked remote keeps its trailing `*`.
    pub name: String,
    /// Commits in `trunk()..@`, less an empty `@`: jj's working-copy
    /// placeholder holds nothing trunk lacks. Any other empty commit counts.
    pub ahead: usize,
    /// Commits in `@..trunk()`: trunk's that I do not have.
    pub behind: usize,
}

/// Flags on the working-copy commit that a prompt should surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// No changes against the parent. jj's answer to "is the tree dirty": an
    /// empty @ is a clean checkout, a non-empty @ already holds the edits.
    pub empty: bool,
    pub conflict: bool,
    pub divergent: bool,
}

/// The working-copy commit, as much of it as the prompt shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Shortest unique prefix of the change id, the part jj highlights.
    pub change_prefix: String,
    /// The remainder of the 8-character change id, shown dimmed.
    pub change_rest: String,
    pub flags: Flags,
    /// Nearest local bookmark at or above `@`, naming the change without
    /// claiming a distance. No `@remote` can appear here.
    pub bookmark: Option<String>,
    /// `None` when the comparison would be meaningless rather than merely
    /// zero: see [`parse`].
    pub trunk: Option<Trunk>,
}

/// Read `@`, the nearest bookmark above it, and its standing against trunk out
/// of the workspace at `root`.
pub fn head(root: &Path) -> Result<Head> {
    // `--ignore-working-copy` keeps the prompt from snapshotting the working
    // copy on every render: a snapshot writes a new operation, fights any jj
    // command running in another pane for the working-copy lock, and costs
    // far more than the read. Verified: op head and `working_copy/tree_state`
    // mtime are both untouched across a render. The trade is that `empty`
    // reflects the last snapshot, so edits made since the last jj command do
    // not light up until the next one.
    let output = Command::new("jj")
        .args(["log", "--repository"])
        .arg(root)
        .args([
            "--ignore-working-copy",
            "--no-graph",
            "--color=never",
            "--quiet",
            "-r",
            &revset(),
            "-T",
            &template(),
        ])
        .output()
        .wrap_err("failed to run `jj log`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("`jj log` failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8(output.stdout).wrap_err("`jj log` wrote non-UTF-8 output")?;
    parse(&stdout)
}

/// Turn the template's rows into a [`Head`].
///
/// The trunk comparison is dropped, rather than rendered as a zero or a
/// whole-history count, in the two cases where it would name nothing a reader
/// could restate: when `trunk()` falls back to `root()` -- which stock jj's
/// alias does in any repository without a `main`/`master` on a remote, and
/// which would then report every commit in the repository as "ahead" -- and
/// when the trunk row is missing entirely.
fn parse(stdout: &str) -> Result<Head> {
    let rows: Vec<Row> = stdout.lines().map(Row::parse).collect();
    let working_copy = rows
        .iter()
        .find(|row| row.tagged('W'))
        .ok_or_else(|| eyre!("`jj log` returned no working-copy commit for `@`"))?;

    let bookmark = rows
        .iter()
        .find(|row| !row.local_bookmarks.is_empty())
        .map(|row| row.local_bookmarks.to_owned());

    let trunk = rows
        .iter()
        .find(|row| row.tagged('T') && !row.tagged('R'))
        .and_then(|row| {
            let name = if row.local_bookmarks.is_empty() {
                row.remote_bookmarks
            } else {
                row.local_bookmarks
            };
            (!name.is_empty()).then(|| Trunk {
                name: name.to_owned(),
                ahead: rows
                    .iter()
                    .filter(|row| row.tagged('A') && !(row.tagged('W') && row.flags.empty))
                    .count(),
                behind: rows.iter().filter(|row| row.tagged('B')).count(),
            })
        });

    Ok(Head {
        change_prefix: working_copy.change_prefix.to_owned(),
        change_rest: working_copy.change_rest.to_owned(),
        flags: working_copy.flags,
        bookmark,
        trunk,
    })
}

/// One rendered template row, borrowed out of the `jj log` output.
struct Row<'a> {
    tags: &'a str,
    change_prefix: &'a str,
    change_rest: &'a str,
    local_bookmarks: &'a str,
    remote_bookmarks: &'a str,
    flags: Flags,
}

impl<'a> Row<'a> {
    /// Split a row on tabs. Missing columns read as empty rather than failing:
    /// the template controls the shape, and a prompt that renders a shorter
    /// segment beats one that renders an error.
    fn parse(line: &'a str) -> Self {
        let mut columns = line.split('\t');
        let tags = columns.next().unwrap_or_default();
        let change_prefix = columns.next().unwrap_or_default();
        let change_rest = columns.next().unwrap_or_default();
        let local_bookmarks = columns.next().unwrap_or_default();
        let remote_bookmarks = columns.next().unwrap_or_default();
        let flags = columns.next().unwrap_or_default();

        Self {
            tags,
            change_prefix,
            change_rest,
            local_bookmarks,
            remote_bookmarks,
            flags: Flags {
                empty: flags.contains('e'),
                conflict: flags.contains('c'),
                divergent: flags.contains('d'),
            },
        }
    }

    fn tagged(&self, tag: char) -> bool {
        self.tags.contains(tag)
    }
}

#[cfg(test)]
mod tests {
    use super::{Flags, Trunk, parse, revset, template};

    /// `W` tags the working copy, `A`/`B` the two sides of the trunk
    /// comparison. `@` is in `trunk()..@`, so its row carries both.
    const ROWS: &str = "\
WA\tykosps\tvq\t\t\t
A\txrlxyz\tum\t\t\t
A\tknkpqw\tsp\t\t\t
TB\touork\tvyw\tmain*\tmain@origin\t
B\tolnqwyy\tk\t\t\t
";

    #[test]
    fn the_arrows_count_revset_membership_not_row_position() {
        let head = parse(ROWS).expect("parse rows");

        assert_eq!(head.change_prefix, "ykosps");
        assert_eq!(head.change_rest, "vq");
        assert_eq!(
            head.trunk,
            Some(Trunk {
                name: "main*".to_owned(),
                ahead: 3,
                behind: 2,
            })
        );
    }

    /// Right after `jj bookmark set main -r @`, jj's fresh empty `@` is the
    /// only member of `trunk()..@`; it is a placeholder, not a commit ahead.
    #[test]
    fn an_empty_working_copy_on_trunk_is_not_ahead() {
        let head = parse("WA\tqp\tzxrtlnk\t\t\te\nT\touork\tvyw\tmain\t\t\n").expect("parse rows");

        assert!(head.flags.empty);
        assert_eq!(
            head.trunk,
            Some(Trunk {
                name: "main".to_owned(),
                ahead: 0,
                behind: 0,
            })
        );
    }

    /// The same shape with edits in `@` is one commit ahead: only the EMPTY
    /// working copy is excluded, not the working copy as such.
    #[test]
    fn a_dirty_working_copy_on_trunk_is_one_ahead() {
        let head = parse("WA\tqp\tzxrtlnk\t\t\t\nT\touork\tvyw\tmain\t\t\n").expect("parse rows");

        assert_eq!(head.trunk.map(|trunk| trunk.ahead), Some(1));
    }

    /// The bug this file exists to prevent: jj's `git` pseudo-remote is a
    /// stale export mirror, and a remote bookmark must never name the change.
    #[test]
    fn a_remote_bookmark_never_names_the_working_copy() {
        let head = parse("W\tyk\tospsvq\t\tmain@git\t\n").expect("parse rows");

        assert_eq!(head.bookmark, None);
    }

    #[test]
    fn a_local_bookmark_names_the_working_copy_without_a_distance() {
        let head = parse("W\tqp\tzxrtlnk\tix-patched\t\tc\n").expect("parse rows");

        assert!(head.flags.conflict);
        assert!(!head.flags.empty);
        assert_eq!(head.bookmark, Some("ix-patched".to_owned()));
    }

    /// Stock jj's `trunk()` alias falls back to `root()`, which would report
    /// the entire repository as "ahead" of it.
    #[test]
    fn a_trunk_that_is_the_root_commit_is_no_comparison_at_all() {
        let head = parse("W\tq\tp\t\t\t\nTRB\tz\tz\t\t\t\n").expect("parse rows");

        assert_eq!(head.trunk, None);
    }

    #[test]
    fn an_unnamed_trunk_is_no_comparison_either() {
        let head = parse("W\tq\tp\t\t\t\nTB\tz\tz\t\t\t\n").expect("parse rows");

        assert_eq!(head.trunk, None);
    }

    #[test]
    fn empty_output_is_an_error_rather_than_a_blank_segment() {
        assert!(parse("").is_err());
    }

    #[test]
    fn flags_survive_a_row_with_every_column_populated() {
        let head = parse("W\ta\tb\tmain\tmain@origin\tecd\n").expect("parse rows");

        assert_eq!(
            head.flags,
            Flags {
                empty: true,
                conflict: true,
                divergent: true,
            }
        );
    }

    /// Every revset the template asks `contained_in` about is also in the
    /// revset being logged, or the tag could never be set on any row.
    #[test]
    fn every_tagged_revset_is_part_of_the_logged_revset() {
        let logged = revset();
        let rendered = template();
        for revset in ["@", "trunk()", "trunk()..@", "@..trunk()"] {
            assert!(
                rendered.contains(&format!(r#"contained_in("{revset}")"#)),
                "template lost the {revset} tag"
            );
            assert!(logged.contains(revset), "revset lost {revset}");
        }
    }
}
