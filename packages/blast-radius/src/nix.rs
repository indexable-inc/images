//! Nix subprocess layer: evaluate `.#checks.x86_64-linux` at a revision and read
//! derivation graphs out of the store.
//!
//! Both are import-from-derivation heavy (the per-unit Cargo graph is rendered
//! by `nix-cargo-unit`, x86_64-linux only), so an end-to-end run needs a Linux
//! builder; `nix-eval-jobs` keeps evaluation memory bounded the way the old
//! nushell tool did.

use std::collections::BTreeMap;
use std::process::Command;

use color_eyre::eyre::{Context, OptionExt, Result, bail};
use serde::Deserialize;

use crate::causes::{DrvNode, Graph};

/// Pinned `nix-eval-jobs` so evaluation behavior does not drift with the user's
/// channels. Matches the revision the old nushell tool used.
const EVAL_JOBS: &str =
    "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7";

/// One evaluated check: its attribute name and the derivation it builds.
pub struct Check {
    pub attr: String,
    pub drv_path: String,
}

/// One line of `nix-eval-jobs` output.
#[derive(Deserialize)]
struct EvalRow {
    attr: String,
    #[serde(default)]
    #[serde(rename = "drvPath")]
    drv_path: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Strip a store path to its derivation name: drop the `/nix/store/<hash>-`
/// prefix and the `.drv` suffix, leaving e.g. `ix-rust-workspace` or
/// `cargo-unit-source-tui-0.1.0-<hash>` -- a stable, readable label.
pub fn drv_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let base = base.strip_suffix(".drv").unwrap_or(base);
    let bytes = base.as_bytes();
    let has_hash_prefix = bytes.len() > 33
        && bytes[32] == b'-'
        && bytes[..32]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if has_hash_prefix {
        base[33..].to_owned()
    } else {
        base.to_owned()
    }
}

/// Run a command, returning stdout on success and a stderr-bearing error on
/// failure. Never swallows stderr: a nonzero exit carries the real reason.
fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("spawn {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{command:?} failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("command stdout was not UTF-8")
}

/// Evaluate every `.#checks.x86_64-linux` derivation at `rev` of the local repo.
///
/// `nix-eval-jobs` sits at the head of the pipeline; a startup/lock/fetch
/// failure surfaces here rather than yielding an empty set that silently
/// under-reports the blast radius.
pub fn eval_checks(repo: &str, rev: &str) -> Result<Vec<Check>> {
    let flakeref = format!("git+file://{repo}?rev={rev}&allRefs=1#checks.x86_64-linux");
    let stdout = run(Command::new("nix").args([
        "run",
        EVAL_JOBS,
        "--",
        "--flake",
        &flakeref,
        "--workers",
        "8",
        "--option",
        "accept-flake-config",
        "true",
        // The base eval predates any `nixConfig` declaration, so enable the
        // content-addressed feature directly rather than via the flake config.
        "--option",
        "extra-experimental-features",
        "ca-derivations",
        // A stale eval cache would mask a real rebuild; force a fresh eval.
        "--option",
        "eval-cache",
        "false",
    ]))
    .with_context(|| format!("evaluate checks at {rev}"))?;

    let mut checks = Vec::new();
    let mut errors = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let row: EvalRow =
            serde_json::from_str(line).with_context(|| format!("parse eval row: {line}"))?;
        // nix-eval-jobs quotes attr segments that need quoting in Nix source
        // (dots, leading digits); strip them so the bare attribute name flows
        // through the diff, the report, and the workflow's safename regex.
        let attr = row.attr.trim_matches('"').to_owned();
        match (row.drv_path, row.error) {
            (Some(drv_path), _) => checks.push(Check { attr, drv_path }),
            (None, Some(error)) => errors.push(format!("{attr}: {error}")),
            (None, None) => {}
        }
    }
    if !errors.is_empty() {
        bail!("checks failed to evaluate at {rev}:\n{}", errors.join("\n"));
    }
    Ok(checks)
}

/// One derivation as `nix derivation show` reports it.
#[derive(Deserialize)]
struct ShowDrv {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "inputDrvs")]
    input_drvs: BTreeMap<String, serde_json::Value>,
}

/// Load the recursive derivation graph rooted at `drv_paths`, keyed by `.drv`
/// store path. Used to walk down to the changed frontier.
pub fn derivation_graph(drv_paths: &[String]) -> Result<Graph> {
    if drv_paths.is_empty() {
        return Ok(Graph::new());
    }
    let mut args = vec![
        "derivation".to_owned(),
        "show".to_owned(),
        "--recursive".to_owned(),
        "--extra-experimental-features".to_owned(),
        "nix-command ca-derivations".to_owned(),
    ];
    args.extend(drv_paths.iter().cloned());
    let stdout = run(Command::new("nix").args(&args)).context("nix derivation show --recursive")?;

    let raw: BTreeMap<String, ShowDrv> =
        serde_json::from_str(&stdout).context("parse nix derivation show output")?;
    Ok(raw
        .into_iter()
        .map(|(path, drv)| {
            let name = drv.name.unwrap_or_else(|| drv_name(&path));
            let inputs = drv.input_drvs.into_keys().collect();
            (path, DrvNode { name, inputs })
        })
        .collect())
}

/// Look up the derivation path for an attribute name in an evaluated set.
pub fn drv_for(checks: &[Check], attr: &str) -> Option<String> {
    checks
        .iter()
        .find(|check| check.attr == attr)
        .map(|check| check.drv_path.clone())
}

/// Resolve the head derivation path for each changed check, skipping any the
/// eval did not surface (should not happen, but never panic on missing data).
pub fn changed_check_drvs(head: &[Check], changed: &[String]) -> Result<BTreeMap<String, String>> {
    changed
        .iter()
        .map(|attr| {
            let drv = drv_for(head, attr)
                .ok_or_eyre(format!("changed check {attr} missing from head eval"))?;
            Ok((attr.clone(), drv))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::drv_name;

    #[test]
    fn drv_name_strips_hash_and_suffix() {
        assert_eq!(
            drv_name("/nix/store/abcdefghijklmnopqrstuvwxyz012345-ix-rust-workspace.drv"),
            "ix-rust-workspace"
        );
        // No hash prefix: left as-is (minus the suffix).
        assert_eq!(drv_name("plain-name.drv"), "plain-name");
        assert_eq!(drv_name("/nix/store/short.drv"), "short");
    }
}
