//! Thin process helpers around the `git` CLI. Every DAG apply-test and the
//! rebase round-trip shell out to real git: the patch series semantics ARE
//! those of `git am` / `rebase --onto` / `format-patch`, so a library
//! reimplementation would be a second opinion, not a port.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

/// Run `git -C <repo> <args>`, returning the raw `Output` whatever the exit
/// status. For callers where a failing exit is a signal (apply-tests), not an
/// error.
pub fn output(repo: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("spawn git -C {} {args:?}", repo.display()))
}

/// Run `git -C <repo> <args>`, failing loudly (with the captured stderr) on a
/// non-zero exit.
pub fn run(repo: &Path, args: &[&str]) -> Result<Output> {
    let out = output(repo, args)?;
    if !out.status.success() {
        bail!(
            "git -C {} {args:?} failed ({}):\n{}",
            repo.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out)
}

/// Run a checked git command and return its trimmed stdout.
pub fn stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let out = run(repo, args)?;
    let text = String::from_utf8(out.stdout)
        .with_context(|| format!("git {args:?} stdout was not UTF-8"))?;
    Ok(text.trim().to_owned())
}

/// A path as `&str` for a git argv slot. Scratch repos and patch dirs are
/// tempdirs and store paths, always UTF-8; a non-UTF-8 path is a loud error,
/// not a lossy mangle.
pub fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("non-UTF-8 path {}", path.display()))
}
