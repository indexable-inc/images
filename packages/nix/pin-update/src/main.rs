//! Update-DAG engine for the repo's pinned artifacts (`nix run .#update`).
//!
//! Nix renders each updater as this binary wrapped over a mode-tagged JSON
//! spec (see `packages/nix/pin-update/default.nix`'s `mkUpdateScript`), so
//! package data stays in Nix and only the machinery lives here. Arguments
//! after the spec belong to the mode (only `claude-code` takes any). Every
//! mode runs from the repo root, which the generated `update` app guarantees.

use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use serde::Deserialize;

mod claude_code;
mod cmd;
mod fork;
mod http;
mod pins;
mod pins_file;
mod pypi;
mod sri;

#[derive(Parser)]
#[command(about = "Refresh pinned artifacts from their upstream coordinates")]
struct Cli {
    /// Mode-tagged JSON spec rendered from Nix.
    spec: PathBuf,
    /// Mode-specific arguments (the claude-code mode's `[version]`,
    /// `--prompts-only`, `--skip-prompts`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
enum Spec {
    Pins(pins::Spec),
    Pypi(pypi::Spec),
    Fork(fork::Spec),
    ClaudeCode(claude_code::Spec),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let raw = std::fs::read_to_string(&cli.spec)
        .with_context(|| format!("reading spec {}", cli.spec.display()))?;
    let spec: Spec = serde_json::from_str(&raw)
        .with_context(|| format!("parsing spec {}", cli.spec.display()))?;

    if !matches!(spec, Spec::ClaudeCode(_)) {
        ensure!(
            cli.args.is_empty(),
            "this updater takes no arguments, got {:?}",
            cli.args
        );
    }
    match spec {
        Spec::Pins(spec) => pins::run(&spec),
        Spec::Pypi(spec) => pypi::run(&spec),
        Spec::Fork(spec) => fork::run(&spec),
        Spec::ClaudeCode(spec) => claude_code::run(&spec, &cli.args),
    }
}

#[cfg(test)]
mod tests {
    use super::Spec;

    #[test]
    fn specs_parse_by_mode_tag() {
        let spec: Spec =
            serde_json::from_str(r#"{"mode": "pins", "pins": "packages/x/pins.json"}"#).unwrap();
        assert!(matches!(spec, Spec::Pins(_)));

        let spec: Spec =
            serde_json::from_str(r#"{"mode": "fork", "fork": "btop", "input": "btop-src"}"#)
                .unwrap();
        assert!(matches!(spec, Spec::Fork(_)));
    }

    #[test]
    fn unknown_spec_fields_are_rejected() {
        let parsed =
            serde_json::from_str::<Spec>(r#"{"mode": "pins", "pins": "p.json", "extra": true}"#);
        let err = parsed.err().expect("extra field should be rejected");
        assert!(err.to_string().contains("extra"));
    }
}
