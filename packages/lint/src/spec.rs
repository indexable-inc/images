//! The generated lint dag spec and the `--json` collector (#1683).
//!
//! default.nix renders the spec; dag-runner executes it. `--json` runs the
//! same nodes directly and emits one JSON document, [{check, ok, output}],
//! so agents can load lint results as a dataframe instead of grepping the
//! human log. dag-runner is bypassed in that mode only because its json mode
//! is an NDJSON event stream that drops the captured diagnostics. The exit
//! code matches the dag-runner contract: the worst stage exit code.

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::thread;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Spec {
    pub nodes: BTreeMap<String, Node>,
}

#[derive(Deserialize)]
pub struct Node {
    pub command: Vec<String>,
}

#[derive(Serialize)]
pub struct StageRun {
    pub check: String,
    pub ok: bool,
    pub output: String,
    #[serde(skip)]
    pub exit_code: i32,
}

/// # Errors
/// When the spec file cannot be read or is not the expected JSON shape.
pub fn load(path: &str) -> Result<Spec> {
    let raw = fs::read_to_string(path).with_context(|| format!("read lint spec {path}"))?;
    serde_json::from_str(&raw).with_context(|| format!("parse lint spec {path}"))
}

/// Run every node concurrently and collect its captured output, in the
/// spec's (sorted) node order.
///
/// # Errors
/// When a node's command cannot be spawned or dies to a signal.
///
/// # Panics
/// When a stage thread itself panics (a bug, not a failing lint).
pub fn run_all(spec: &Spec) -> Result<Vec<StageRun>> {
    thread::scope(|scope| {
        // Spawn everything before joining anything: a spawn-then-join map
        // chain would serialize the stages.
        let mut handles = Vec::with_capacity(spec.nodes.len());
        for (check, node) in &spec.nodes {
            handles.push((check, scope.spawn(move || run_node(check, node))));
        }
        handles
            .into_iter()
            .map(|(check, handle)| {
                handle
                    .join()
                    .unwrap_or_else(|_| panic!("lint stage thread panicked: {check}"))
            })
            .collect()
    })
}

fn run_node(check: &str, node: &Node) -> Result<StageRun> {
    let (program, args) = node
        .command
        .split_first()
        .with_context(|| format!("lint spec node {check} has an empty command"))?;
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run lint stage {check}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    let exit_code = output
        .status
        .code()
        .with_context(|| format!("lint stage {check} was killed by a signal"))?;
    Ok(StageRun {
        check: check.to_owned(),
        ok: exit_code == 0,
        // Strip ANSI: the stages color their diagnostics, and raw ESC bytes
        // in JSON strings would be rejected by strict parsers (jq).
        output: strip_ansi_escapes::strip_str(&combined),
        exit_code,
    })
}

#[must_use]
pub fn worst_exit_code(runs: &[StageRun]) -> i32 {
    runs.iter().map(|run| run.exit_code).max().unwrap_or(0)
}

/// # Errors
/// When serialization fails (a captured output is not valid UTF-8 JSON).
pub fn to_json(runs: &[StageRun]) -> Result<String> {
    serde_json::to_string_pretty(runs).context("serialize lint runs")
}

/// The dag spec's stage list must equal the binary's own [`crate::stage::Stage`]
/// enum; `lint-stage --list` exposes the enum for the nix-side check.
///
/// # Errors
/// When the spec's nodes and the built-in stage list diverge.
pub fn validate_stages(spec: &Spec, stages: &[String]) -> Result<()> {
    let spec_stages: Vec<&String> = spec.nodes.keys().collect();
    let mut sorted: Vec<&String> = stages.iter().collect();
    sorted.sort();
    ensure!(
        spec_stages == sorted,
        "lint spec nodes {spec_stages:?} do not match the built-in stages {sorted:?}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parses_and_orders_nodes() {
        let spec: Spec = serde_json::from_str(
            r#"{"nodes": {"statix": {"command": ["/bin/lint-stage", "statix"]},
                          "alejandra": {"command": ["/bin/lint-stage", "alejandra"]}}}"#,
        )
        .expect("spec fixture parses");
        let names: Vec<&String> = spec.nodes.keys().collect();
        assert_eq!(names, ["alejandra", "statix"]);
    }

    #[test]
    fn worst_exit_code_is_the_max() {
        let runs = vec![
            StageRun {
                check: "a".to_owned(),
                ok: true,
                output: String::new(),
                exit_code: 0,
            },
            StageRun {
                check: "b".to_owned(),
                ok: false,
                output: String::new(),
                exit_code: 2,
            },
        ];
        assert_eq!(worst_exit_code(&runs), 2);
        assert_eq!(worst_exit_code(&[]), 0);
    }

    #[test]
    fn json_shape_drops_exit_code() {
        let runs = vec![StageRun {
            check: "a".to_owned(),
            ok: false,
            output: "boom".to_owned(),
            exit_code: 1,
        }];
        let json = to_json(&runs).expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(
            value,
            serde_json::json!([{"check": "a", "ok": false, "output": "boom"}])
        );
    }
}
