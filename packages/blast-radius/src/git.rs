//! Revision resolution. The base is the merge-base of `origin/main` (or the
//! caller's base) with the head, so the report reflects only what this branch
//! changed, not commits that landed on main in the meantime.

use std::process::Command;

use color_eyre::eyre::{Context, Result, bail};

/// The repository root plus the resolved base and head commit SHAs.
pub struct Revs {
    pub repo: String,
    pub base: String,
    pub head: String,
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .with_context(|| format!("spawn git {args:?}"))?;
    if !output.status.success() {
        bail!(
            "git {args:?} failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)
        .context("git stdout was not UTF-8")?
        .trim()
        .to_owned())
}

/// Resolve the repo root and the base/head commits to diff. `base` defaults to
/// `origin/main` and `head` to `HEAD`.
pub fn resolve(base: Option<&str>, head: Option<&str>) -> Result<Revs> {
    let repo = git(&["rev-parse", "--show-toplevel"])?;
    let head_rev = git(&[
        "rev-parse",
        "--verify",
        &format!("{}^{{commit}}", head.unwrap_or("HEAD")),
    ])?;
    let base_in = git(&[
        "rev-parse",
        "--verify",
        &format!("{}^{{commit}}", base.unwrap_or("origin/main")),
    ])?;
    let base_rev = git(&["merge-base", &base_in, &head_rev])?;
    Ok(Revs {
        repo,
        base: base_rev,
        head: head_rev,
    })
}
