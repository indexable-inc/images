//! How long ago the checkout's latest commit landed, for either VCS.
//!
//! The git half is what starship's config used to run inline (`git log -1
//! --pretty=format:%cr`). It moved here when `require_repo` learned about jj:
//! the module is now enabled inside a jj workspace, where `git log` has no
//! repository to read and the segment would have gone blank.

use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr};

use crate::workspace::Workspace;

/// `@-`, not `@`: `@` is the working-copy commit, which jj recreates on every
/// `jj new`, so its timestamp answers "when did I start this change" rather
/// than "when did the last commit land". `@-` is also what `jj git export`
/// writes to `HEAD`, so a colocated repo reports the same instant here as the
/// git branch below.
const REVISION: &str = "@-";

/// `ago()` renders the same vocabulary as git's `%cr` ("13 minutes ago"). The
/// `root` guard suppresses the root commit's epoch timestamp, which a
/// repository with no commits yet would otherwise render as "53 years ago".
const TEMPLATE: &str = r#"if(root, "", committer.timestamp().ago()) ++ "\n""#;

/// Time since the latest commit, or `None` when there is no commit to date:
/// an empty repository, or a checkout whose latest commit is the root.
pub fn since_last_commit(workspace: &Workspace) -> Result<Option<String>> {
    let stdout = match workspace {
        Workspace::Jj(root) => jj(root)?,
        Workspace::Git(root) => git(root)?,
    };
    Ok(stdout.and_then(|stdout| first_line(&stdout).map(str::to_owned)))
}

fn jj(root: &Path) -> Result<Option<String>> {
    // `--ignore-working-copy` for the same reason `jj::head` uses it: a prompt
    // must not snapshot the working copy, which writes an operation and fights
    // any jj command running in another pane for the working-copy lock.
    let output = Command::new("jj")
        .args(["log", "--repository"])
        .arg(root)
        .args([
            "--ignore-working-copy",
            "--no-graph",
            "--color=never",
            "--quiet",
            "-r",
            REVISION,
            "-T",
            TEMPLATE,
        ])
        .output()
        .wrap_err("failed to run `jj log`")?;

    // A failure here is not worth an error: the common cause is a workspace
    // whose only commit is the root, where `@-` does not resolve, and the
    // right answer for the prompt is an empty segment either way.
    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(
        String::from_utf8(output.stdout).wrap_err("`jj log` wrote non-UTF-8 output")?,
    ))
}

fn git(root: &Path) -> Result<Option<String>> {
    // `--no-optional-locks` for the same reason `git::head` uses it: rendering
    // a prompt must not take the index lock to refresh caches.
    let output = Command::new("git")
        .args(["--no-optional-locks", "-C"])
        .arg(root)
        .args(["log", "-1", "--pretty=format:%cr"])
        .output()
        .wrap_err("failed to run `git log`")?;

    // Exits non-zero on a repository with no commits, which is not an error
    // here for the same reason as above.
    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(
        String::from_utf8(output.stdout).wrap_err("`git log` wrote non-UTF-8 output")?,
    ))
}

/// The first line with anything on it. `@-` can name several commits when the
/// working copy is a merge, and the template renders the root commit as an
/// empty line; either way the prompt shows one age.
fn first_line(stdout: &str) -> Option<&str> {
    stdout.lines().map(str::trim).find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::first_line;

    #[test]
    fn takes_the_first_line_with_content() {
        assert_eq!(first_line("13 minutes ago\n"), Some("13 minutes ago"));
    }

    #[test]
    fn skips_the_blank_line_a_root_commit_renders() {
        assert_eq!(first_line("\n2 days ago\n"), Some("2 days ago"));
    }

    #[test]
    fn a_merge_reports_one_age_rather_than_several() {
        assert_eq!(first_line("2 days ago\n5 weeks ago\n"), Some("2 days ago"));
    }

    #[test]
    fn nothing_but_blank_lines_is_no_age() {
        assert_eq!(first_line("\n \n"), None);
    }
}
