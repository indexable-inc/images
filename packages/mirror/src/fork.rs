//! `mirror fork-branch`: build a de-forked package's `ix-patched` branch from
//! declarative data — the upstream base pinned in `flake.lock` plus the
//! in-repo patch series (lib/fork-packages.nix) applied as real commits — and
//! optionally force-push it to the org's fork repo. Without `--push` this is
//! a pure verification: the series either applies cleanly or fails loudly.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::exec;
use crate::workspace::Workspace;

pub const BRANCH: &str = "ix-patched";

pub struct Request {
    /// Fork name from lib/fork-packages.nix, e.g. `codex`.
    pub name: String,
    pub push: bool,
    pub mapping: Option<PathBuf>,
}

pub fn run(workspace: &Workspace, request: &Request) -> Result<()> {
    let forks = fork_mapping(workspace, request.mapping.as_deref())?;
    let fork = forks
        .iter()
        .find(|fork| fork.get("name").and_then(Value::as_str) == Some(&request.name))
        .with_context(|| {
            let known: Vec<&str> = forks
                .iter()
                .filter_map(|fork| fork.get("name").and_then(Value::as_str))
                .collect();
            format!(
                "no fork named `{}` (known: {})",
                request.name,
                known.join(", ")
            )
        })?;
    let field = |key: &str| {
        fork.get(key)
            .and_then(Value::as_str)
            .with_context(|| format!("fork `{}` has no `{key}`", request.name))
    };
    let url = field("url")?;
    let rev = locked_rev(&workspace.root.join("flake.lock"), field("input")?)?;
    let patches = patch_series(&workspace.root.join(field("patchDir")?))?;

    let scratch = tempfile::tempdir().context("creating scratch directory")?;
    let repo = scratch.path();
    exec::git(repo, &["init", "--quiet"])?;
    // Full (non-shallow) fetch of the pinned base: the fork repo needs real
    // history for upstream PRs to be openable from it.
    exec::git(repo, &["fetch", "--quiet", url, &rev])?;
    exec::git(repo, &["checkout", "--quiet", "-B", BRANCH, &rev])?;

    let mut args = vec![
        "-c",
        "user.name=index-mirror",
        "-c",
        "user.email=index-mirror@users.noreply.github.com",
        "am",
        "--3way",
    ];
    args.extend(patches.iter().map(String::as_str));
    exec::git(repo, &args)
        .with_context(|| format!("applying the `{}` patch series onto {rev}", request.name))?;
    println!(
        "{}: {} patches apply cleanly on {url}@{rev}",
        request.name,
        patches.len()
    );

    if request.push {
        let fork_repo = fork
            .get("forkRepo")
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "fork `{}` has no `forkRepo` in lib/fork-packages.nix; add one to push",
                    request.name
                )
            })?;
        let push_url =
            crate::publish::authenticated_url(&format!("https://github.com/{fork_repo}.git"));
        // Force: a rebased series rewrites the branch by design.
        exec::git(
            repo,
            &[
                "push",
                "--force",
                "--quiet",
                &push_url,
                &format!("{BRANCH}:{BRANCH}"),
            ],
        )?;
        println!("{}: pushed {BRANCH} to {fork_repo}", request.name);
    }
    Ok(())
}

fn fork_mapping(workspace: &Workspace, json: Option<&Path>) -> Result<Vec<Value>> {
    let text = match json {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => exec::run(
            &workspace.root,
            "nix",
            &["eval", "--json", ".#lib.forkPackages"],
        )?,
    };
    let value: Value = serde_json::from_str(&text).context("parsing forkPackages JSON")?;
    value
        .as_array()
        .cloned()
        .context("forkPackages JSON is not a list")
}

fn locked_rev(flake_lock: &Path, input: &str) -> Result<String> {
    let text = fs::read_to_string(flake_lock)
        .with_context(|| format!("reading {}", flake_lock.display()))?;
    let value: Value = serde_json::from_str(&text).context("parsing flake.lock")?;
    value
        .pointer(&format!("/nodes/{input}/locked/rev"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .with_context(|| format!("flake.lock has no locked rev for input `{input}`"))
}

/// The `*.patch` files of a series directory in natural order (digit runs
/// compare numerically, so `2-x.patch` sorts before `10-x.patch`).
fn patch_series(dir: &Path) -> Result<Vec<String>> {
    let mut patches = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "patch")
        {
            patches.push(path.to_str().context("patch path is not UTF-8")?.to_owned());
        }
    }
    if patches.is_empty() {
        bail!("{} contains no *.patch files", dir.display());
    }
    patches.sort_by(|a, b| natural_cmp(a, b));
    Ok(patches)
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let ordering = take_number(&mut a).cmp(&take_number(&mut b));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u128 {
    let mut number: u128 = 0;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        number = number.saturating_mul(10).saturating_add(u128::from(digit));
        chars.next();
    }
    number
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_compares_digit_runs_numerically() {
        let mut series = vec!["0010-b.patch", "0002-a.patch", "0001-z.patch"];
        series.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(series, ["0001-z.patch", "0002-a.patch", "0010-b.patch"]);
        assert_eq!(natural_cmp("2-x.patch", "10-x.patch"), Ordering::Less);
    }
}
