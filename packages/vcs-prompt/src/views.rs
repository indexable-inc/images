//! Which configured jj view owns the prompt directory, and where it stands.
//!
//! `jj views prompt` is the seam: it maps the directory to the repository's
//! `views` config table and reads the survey record the last `jj views fetch`
//! or `jj views status` left. The counts are therefore as fresh as the last
//! survey and cost a config read plus a file read, where computing them fresh
//! is a derive over the view's whole history -- seconds, which no prompt can
//! pay.
//!
//! Because they are a cache, they need a vintage. `jj views prompt` prints
//! the counts alone, and a count without the time it was taken is the kind of
//! number a reader restates wrongly: the record behind `ix⇣241` here was
//! 7h41m old, and its surveyed upstream had itself moved 33 commits on, so
//! 241 was already wrong low. The record file's mtime is the cheapest honest
//! vintage available -- one `stat`, no jj -- so the segment carries it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// The view the prompt directory sits inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub name: String,
    /// Counts from the last survey; `None` for a view never surveyed.
    pub counts: Option<Counts>,
    /// Age of the survey record, `None` when it cannot be read.
    pub age: Option<Duration>,
}

/// How the view stood against its published repository at the last survey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Published commits that had not arrived here.
    pub behind: usize,
    /// View commits here the published repository did not have.
    pub ahead: usize,
}

/// The view owning `cwd` in the workspace at `root`, or `None` outside every
/// view.
///
/// Every failure is also `None`: stock jj exits nonzero on the unknown
/// subcommand, and a segment that goes missing beats one that renders an
/// error. The working-copy state still renders either way, so a missing view
/// segment stays visible as exactly that.
pub fn at(root: &Path, cwd: &Path) -> Option<View> {
    let output = Command::new("jj")
        .args(["views", "prompt", "--repository"])
        .arg(root)
        .args(["--ignore-working-copy", "--color=never", "--quiet"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut view = parse(&String::from_utf8(output.stdout).ok()?)?;
    view.age = record_age(root, &view.name);
    Some(view)
}

/// One `name<TAB>behind<TAB>ahead` line, or bare `name` for a view never
/// surveyed, or nothing at all outside every view.
fn parse(stdout: &str) -> Option<View> {
    let line = stdout.lines().next()?;
    let mut columns = line.split('\t');
    let name = columns.next().filter(|name| !name.is_empty())?.to_owned();
    let counts = match (columns.next(), columns.next()) {
        (Some(behind), Some(ahead)) => Some(Counts {
            behind: behind.parse().ok()?,
            ahead: ahead.parse().ok()?,
        }),
        _ => None,
    };
    Some(View {
        name,
        counts,
        age: None,
    })
}

/// How long ago the last survey of `name` wrote its record.
fn record_age(root: &Path, name: &str) -> Option<Duration> {
    let modified = fs::metadata(record_path(root, name)?).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// `<repo>/views/<name>.json`, where the repository directory is `.jj/repo`
/// for the default workspace and the path *named by* that file for every
/// other one -- a secondary workspace's `.jj/repo` is a pointer, not the
/// store, so reading it as a directory finds nothing and would report every
/// record as missing.
fn record_path(root: &Path, name: &str) -> Option<PathBuf> {
    let repo = root.join(".jj").join("repo");
    let repo = if repo.is_dir() {
        repo
    } else {
        PathBuf::from(fs::read_to_string(&repo).ok()?.trim())
    };
    Some(repo.join("views").join(format!("{name}.json")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::{Counts, View, parse, record_path};

    #[test]
    fn a_surveyed_view_carries_its_counts() {
        assert_eq!(
            parse("ix\t25\t1\n"),
            Some(View {
                name: "ix".to_owned(),
                counts: Some(Counts {
                    behind: 25,
                    ahead: 1,
                }),
                age: None,
            })
        );
    }

    #[test]
    fn a_view_never_surveyed_is_a_bare_name() {
        assert_eq!(
            parse("ix\n"),
            Some(View {
                name: "ix".to_owned(),
                counts: None,
                age: None,
            })
        );
    }

    #[test]
    fn outside_every_view_there_is_nothing_to_parse() {
        assert_eq!(parse(""), None);
    }

    #[test]
    fn the_default_workspace_keeps_its_record_under_dot_jj_repo() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".jj/repo")).expect("create repo dir");

        assert_eq!(
            record_path(root.path(), "ix"),
            Some(root.path().join(".jj/repo/views/ix.json"))
        );
    }

    /// A secondary workspace's `.jj/repo` is a file naming the real store.
    #[test]
    fn a_secondary_workspace_follows_its_repo_pointer() {
        let root = tempfile::tempdir().expect("tempdir");
        let store = tempfile::tempdir().expect("store tempdir");
        fs::create_dir_all(root.path().join(".jj")).expect("create .jj");
        fs::write(
            root.path().join(".jj/repo"),
            format!("{}\n", store.path().display()),
        )
        .expect("write pointer");

        assert_eq!(
            record_path(root.path(), "ix"),
            Some(store.path().join("views/ix.json"))
        );
    }

    #[test]
    fn a_record_that_is_not_there_has_no_age() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".jj/repo")).expect("create repo dir");

        assert_eq!(super::record_age(root.path(), "ix"), None);
    }

    #[test]
    fn a_record_just_written_is_fresh() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join(".jj/repo/views")).expect("create views dir");
        fs::write(root.path().join(".jj/repo/views/ix.json"), "{}").expect("write record");

        let age = super::record_age(root.path(), "ix").expect("an age");
        assert!(age < Duration::from_mins(1), "fresh record read as {age:?}");
    }
}
