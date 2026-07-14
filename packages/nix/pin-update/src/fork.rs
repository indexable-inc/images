//! Fork-base bump for a de-forked package: advance the flake input its
//! upstream base is pinned to, then regenerate the in-repo patch series.
//! `rebase-patches` no-ops when the rev did not move and fails loudly (naming
//! the conflicting patch) on an unresolved rebase.

use std::process::Command;

use anyhow::Result;
use serde::Deserialize;

use crate::cmd;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Fork name: matches `lib/fork-packages.nix` and the rebase-patches arg.
    fork: String,
    /// The flake.lock input the bump advances.
    input: String,
}

pub fn run(spec: &Spec) -> Result<()> {
    cmd::run(Command::new("nix").args(["flake", "update"]).arg(&spec.input))?;
    cmd::run(Command::new("rebase-patches").arg(&spec.fork))
}
