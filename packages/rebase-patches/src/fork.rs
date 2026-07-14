//! The fork-package mapping: data from lib/fork-packages.nix rendered to JSON
//! and baked into the wrapped binary as [`MAPPING_ENV`] (a store path), so the
//! code hardcodes no per-package coordinates. A downstream repo (e.g. ix) that
//! keeps its own fork mapping + patches reuses this one tool by pointing
//! `--mapping` at its list, run from its repo root so `patchDir` and
//! `flake.lock` resolve there. One tool, parameterized by data, never copied.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

/// Env var the nix wrapper points at the baked-in fork mapping JSON.
pub const MAPPING_ENV: &str = "REBASE_PATCHES_FORK_MAPPING";

/// One fork package from the mapping JSON (lib/fork-packages.nix shape).
#[derive(Debug, Clone, Deserialize)]
pub struct Fork {
    pub name: String,
    /// Flake input whose `locked.rev` pins the upstream base.
    pub input: String,
    /// Upstream git URL bases are fetched from.
    pub url: String,
    /// Patch dir, repo-relative; resolves against the invocation cwd (the repo
    /// root).
    #[serde(rename = "patchDir")]
    pub patch_dir: String,
}

impl Fork {
    /// The patch dir pinned to an absolute path up front, since the tool works
    /// inside scratch repos elsewhere on disk.
    pub fn patch_dir_abs(&self) -> Result<PathBuf> {
        std::path::absolute(&self.patch_dir)
            .with_context(|| format!("resolve patch dir {}", self.patch_dir))
    }
}

/// The mapping to drive: the caller-supplied `--mapping` path (a downstream
/// repo pointing this one tool at its own fork list) else the baked-in list
/// from the wrapper env.
fn mapping_path(mapping: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = mapping {
        return Ok(path.to_owned());
    }
    std::env::var_os(MAPPING_ENV).map(PathBuf::from).with_context(|| {
        format!("rebase-patches: no --mapping given and {MAPPING_ENV} unset (not the wrapped nix package?)")
    })
}

/// Resolve the selected fork records from an optional name against the mapping.
pub fn select(name: Option<&str>, mapping: Option<&Path>) -> Result<Vec<Fork>> {
    let path = mapping_path(mapping)?;
    let text = fs::read_to_string(&path)
        .with_context(|| format!("read fork mapping {}", path.display()))?;
    let forks: Vec<Fork> = serde_json::from_str(&text)
        .with_context(|| format!("parse fork mapping {}", path.display()))?;

    let Some(name) = name else { return Ok(forks) };
    let hit: Vec<Fork> = forks.iter().filter(|f| f.name == name).cloned().collect();
    if hit.is_empty() {
        let known: Vec<&str> = forks.iter().map(|f| f.name.as_str()).collect();
        bail!(
            "rebase-patches: no fork package named {name}; known: {}",
            known.join(", ")
        );
    }
    Ok(hit)
}

/// The pinned base rev of a flake input from a parsed `flake.lock`.
pub fn locked_rev(lock: &Value, input: &str) -> Result<String> {
    lock["nodes"][input]["locked"]["rev"]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("flake.lock has no nodes.{input}.locked.rev"))
}
