//! Access to the rendered `.#lib.mirrorPackages` manifest: the declarative
//! per-package mirror metadata (repo, description, topics, flake attr) that
//! `mirror` attrs in package.nix render to. The single source of truth for
//! everything curated about a mirror; the generator derives the rest.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::exec;
use crate::workspace::Workspace;

pub struct Entry {
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub path: String,
    /// Mirror repo `owner/name`.
    pub repo: String,
    pub description: Option<String>,
    pub topics: Vec<String>,
    /// Monorepo flake output attr when the package is flake-exposed.
    pub flake_attr: Option<String>,
}

/// The manifest entry for `package`, from `json` when given, else by
/// rendering `.#lib.mirrorPackages` with `nix eval`. Loud when the package
/// has no entry: without one there is no mirror to generate for.
pub fn entry_for(workspace: &Workspace, package: &Path, json: Option<&Path>) -> Result<Entry> {
    let package = package.to_str().context("package path is not UTF-8")?;
    let package = package.trim_end_matches('/');
    load(workspace, json)?
        .into_iter()
        .find(|entry| entry.path == package)
        .with_context(|| format!("`{package}` has no `mirror` attr in its package.nix"))
}

fn load(workspace: &Workspace, json: Option<&Path>) -> Result<Vec<Entry>> {
    let text = match json {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => exec::run(
            &workspace.root,
            "nix",
            &["eval", "--json", ".#lib.mirrorPackages"],
        )?,
    };
    let value: Value = serde_json::from_str(&text).context("parsing mirrorPackages JSON")?;
    value
        .as_array()
        .context("mirrorPackages JSON is not a list")?
        .iter()
        .map(parse)
        .collect()
}

fn parse(value: &Value) -> Result<Entry> {
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(Entry {
        path: field("path").context("mirror entry without `path`")?,
        repo: field("repo").context("mirror entry without `repo`")?,
        description: field("description"),
        flake_attr: field("flakeAttr"),
        topics: value
            .get("topics")
            .and_then(Value::as_array)
            .map(|topics| {
                topics
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}
