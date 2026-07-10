//! Captured-output subprocess runner: every git/gh/nix interaction goes
//! through one function so a failure always names the command and its stderr,
//! with any `MIRROR_TOKEN` value redacted before it can reach a log.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::workspace::Workspace;

/// Run `program args..` in `dir` and return its trimmed stdout. A non-zero
/// exit becomes an error carrying the command line and stderr.
pub fn run(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("spawning `{program}` (is it installed?)"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    bail!(
        "`{program} {}` failed in {}: {}",
        redact(&args.join(" ")),
        dir.display(),
        redact(String::from_utf8_lossy(&output.stderr).trim())
    );
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    run(dir, "git", args)
}

pub fn nix_json(workspace: &Workspace, file: Option<&Path>, attribute: &str) -> Result<Value> {
    let text = match file {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => run(
            &workspace.root,
            "nix",
            &["eval", "--json", &format!(".#lib.{attribute}")],
        )?,
    };
    serde_json::from_str(&text).with_context(|| format!("parsing {attribute} JSON"))
}

/// Strip the push token out of a string destined for an error message, so an
/// authenticated remote URL can never leak through a failure report.
fn redact(text: &str) -> String {
    std::env::var("MIRROR_TOKEN").map_or_else(
        |_| text.to_owned(),
        |token| {
            if token.is_empty() {
                text.to_owned()
            } else {
                text.replace(&token, "***")
            }
        },
    )
}
