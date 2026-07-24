//! Working-copy state of a jj workspace, read with one `jj log` call.
//!
//! `jj-lib` would answer the same questions in-process, but it is a heavy
//! dependency to build and pin (it is why the third-party `jj-starship` was
//! dropped from this config), and the CLI answers in ~20ms on the largest
//! workspace here, which a prompt can pay.

use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr, eyre};

/// How far back to look for a bookmark to name the working copy after. jj
/// bookmarks do not move on commit, so the useful answer is almost always a
/// few commits back; the cap keeps `jj log` from walking a long unbookmarked
/// history and keeps the parse bounded.
const BOOKMARK_SEARCH_DEPTH: usize = 20;

/// One tab-separated row per commit: change-id prefix, the rest of the
/// shortest unique change id, comma-joined local bookmarks, then a flag blob.
/// Fields are concatenated rather than passed through the template's
/// `separate()`, which drops empty values and would shift the columns.
const TEMPLATE: &str = concat!(
    r#"change_id.shortest(8).prefix() ++ "\t""#,
    r#" ++ change_id.shortest(8).rest() ++ "\t""#,
    r#" ++ bookmarks.join(",") ++ "\t""#,
    r#" ++ if(empty, "e", "") ++ if(conflict, "c", "") ++ if(divergent, "d", "") ++ "\n""#,
);

/// The bookmark the working copy hangs off, and how far ahead of it @ sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// Comma-joined bookmark names exactly as jj renders them, so a bookmark
    /// out of sync with its tracked remote keeps its trailing `*`.
    pub names: String,
    /// Commits between the bookmark and @; 0 when @ carries the bookmark.
    pub distance: usize,
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
    pub bookmark: Option<Bookmark>,
}

/// Read @ and the nearest bookmark above it out of the workspace at `root`.
pub fn head(root: &Path) -> Result<Head> {
    // `--ignore-working-copy` keeps the prompt from snapshotting the working
    // copy on every render: a snapshot writes a new operation, fights any jj
    // command running in another pane for the working-copy lock, and costs
    // far more than the read. The trade is that `empty` reflects the last
    // snapshot, so edits made since the last jj command do not light up until
    // the next one.
    let output = Command::new("jj")
        .args(["log", "--repository"])
        .arg(root)
        .args([
            "--ignore-working-copy",
            "--no-graph",
            "--color=never",
            "--quiet",
            "-r",
            &format!("ancestors(@, {BOOKMARK_SEARCH_DEPTH})"),
            "-T",
            TEMPLATE,
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

/// Turn the template's rows into a [`Head`]. Rows arrive newest-first, so the
/// first one is @ and the index of the first bookmarked row is its distance.
fn parse(stdout: &str) -> Result<Head> {
    let rows: Vec<Row> = stdout.lines().map(Row::parse).collect();
    let working_copy = rows
        .first()
        .ok_or_else(|| eyre!("`jj log` returned no commits for `ancestors(@)`"))?;

    let bookmark = rows
        .iter()
        .enumerate()
        .find(|(_, row)| !row.bookmarks.is_empty())
        .map(|(distance, row)| Bookmark {
            names: row.bookmarks.to_owned(),
            distance,
        });

    Ok(Head {
        change_prefix: working_copy.change_prefix.to_owned(),
        change_rest: working_copy.change_rest.to_owned(),
        flags: working_copy.flags,
        bookmark,
    })
}

/// One rendered template row, borrowed out of the `jj log` output.
struct Row<'a> {
    change_prefix: &'a str,
    change_rest: &'a str,
    bookmarks: &'a str,
    flags: Flags,
}

impl<'a> Row<'a> {
    /// Split a row on tabs. Missing columns read as empty rather than failing:
    /// the template controls the shape, and a prompt that renders a shorter
    /// segment beats one that renders an error.
    fn parse(line: &'a str) -> Self {
        let mut columns = line.split('\t');
        let change_prefix = columns.next().unwrap_or_default();
        let change_rest = columns.next().unwrap_or_default();
        let bookmarks = columns.next().unwrap_or_default();
        let flags = columns.next().unwrap_or_default();

        Self {
            change_prefix,
            change_rest,
            bookmarks,
            flags: Flags {
                empty: flags.contains('e'),
                conflict: flags.contains('c'),
                divergent: flags.contains('d'),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bookmark, Flags, parse};

    #[test]
    fn reads_the_working_copy_and_the_nearest_bookmark() {
        let head = parse("l\tsurukvy\t\te\nmw\typryqr\tix-patched\te\no\towsttwz\t\t\n")
            .expect("parse rows");

        assert_eq!(head.change_prefix, "l");
        assert_eq!(head.change_rest, "surukvy");
        assert_eq!(
            head.flags,
            Flags {
                empty: true,
                ..Flags::default()
            }
        );
        assert_eq!(
            head.bookmark,
            Some(Bookmark {
                names: "ix-patched".to_owned(),
                distance: 1,
            })
        );
    }

    #[test]
    fn a_bookmark_on_the_working_copy_is_zero_away() {
        let head = parse("qp\tzxrtlnk\tmain*\tc\n").expect("parse rows");

        assert!(head.flags.conflict);
        assert!(!head.flags.empty);
        assert_eq!(head.bookmark.expect("bookmark").distance, 0);
    }

    #[test]
    fn no_bookmark_within_the_window_leaves_the_change_id_alone() {
        let head = parse("qp\tzxrtlnk\t\t\nyy\tnmtruop\t\t\n").expect("parse rows");

        assert_eq!(head.bookmark, None);
    }

    #[test]
    fn empty_output_is_an_error_rather_than_a_blank_segment() {
        assert!(parse("").is_err());
    }
}
