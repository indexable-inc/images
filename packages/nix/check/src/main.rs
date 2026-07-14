//! `check`: the full CI gate as one repo-owned command (check.yml runs
//! `nix run .#check`, so the same steps run in CI and locally from a single
//! definition). See default.nix for the gate's architecture notes; this
//! binary owns the mechanics.
//!
//! Step 1 (nix-fast-build over `.#ciChecks.x86_64-linux`) is the build gate:
//! parallel eval via nix-eval-jobs, each drv streamed into a build pool,
//! nonzero iff a build or eval fails. Step 2 (nix-eval-jobs over
//! `.#packages.x86_64-linux`) is the schema/eval gate, broader than the
//! checks set step 1 built; nix-eval-jobs reports a per-attribute eval
//! failure as a JSON `error` line and still exits 0, so the gate is the
//! error-line scan, while a startup or lock failure exits nonzero and aborts
//! the run. `check closure` (closure-gate.yml, #1873) reuses step 1's build
//! gate over `.#cachePushRoots.x86_64-linux`. `nix` comes from the ambient
//! PATH on purpose (this is always invoked as `nix run .#check`, so the
//! host's daemon-matched nix is already present); the two evaluator binaries
//! arrive by store path in `IX_NIX_FAST_BUILD` / `IX_NIX_EVAL_JOBS` from the
//! wrapper built in default.nix.

use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;

const CI_CHECKS_FLAKE: &str = ".#ciChecks.x86_64-linux";
const CLOSURE_FLAKE: &str = ".#cachePushRoots.x86_64-linux";
const RESULT_FILE: &str = "check-results.json";
/// GitHub check-run annotations truncate long messages anyway; keep the
/// `::error::` payloads bounded well below the API limit.
const ANNOTATION_TAIL_CHARS: usize = 600;
const ANNOTATION_ERROR_CHARS: usize = 500;

/// Run the full CI gate: build .#ciChecks.x86_64-linux and eval-validate
/// .#packages.x86_64-linux.
#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Pre-merge closure gate (closure-gate.yml, #1873): the same build gate
    /// over the roots the post-merge cache-push linux lane publishes, darwin
    /// cross closure included -- the set #2690 broke while flake-check stayed
    /// green (packages are eval-gated only). --skip-cached keeps it
    /// O(changed): on the warm-store pool only drvs new relative to main's
    /// already-built closure realise.
    Closure,
}

/// The two repo-built evaluator binaries, injected by the wrapper.
struct Tools {
    fast_build: String,
    eval_jobs: String,
}

impl Tools {
    fn from_env() -> Result<Self> {
        let read = |name: &str| {
            env::var(name)
                .with_context(|| format!("{name} is not set; run check via its wrapped package"))
        };
        Ok(Self {
            fast_build: read("IX_NIX_FAST_BUILD")?,
            eval_jobs: read("IX_NIX_EVAL_JOBS")?,
        })
    }
}

#[derive(Deserialize)]
struct ResultFile {
    results: Vec<CheckRecord>,
}

/// One nix-fast-build --result-file record: {attr, type: EVAL|BUILD,
/// duration, success, error, outputs} per attr per phase.
#[derive(Deserialize)]
struct CheckRecord {
    attr: String,
    #[serde(rename = "type")]
    phase: String,
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

struct Captured {
    success: bool,
    stdout: String,
    stderr: String,
}

fn capture(command: &mut Command) -> Result<Captured> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().with_context(|| format!("run {program}"))?;
    Ok(Captured {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// The last `count` lines joined with " | ", bounded for an annotation.
fn tail_annotation(lines: &[&str], count: usize) -> String {
    let start = lines.len().saturating_sub(count);
    truncate_chars(&lines[start..].join(" | "), ANNOTATION_TAIL_CHARS)
}

/// Shared build gate: build every derivation under `flake` with
/// nix-fast-build and return failure after replaying each failed build's
/// log. `main` runs it over ciChecks; `closure` over the cache-push roots.
fn build_gate(tools: &Tools, flake: &str) -> Result<ExitCode> {
    // ca-derivations: the rust workspace units default to
    // `contentAddressed = true` (lib/rust/cargo-unit.nix), so evaluating
    // the target set resolves floating content-addressed drvs. The
    // evaluator (nix-eval-jobs, which nix-fast-build wraps) needs the
    // `ca-derivations` experimental feature, or it aborts with
    // "experimental Nix feature 'ca-derivations' is disabled". The caller
    // owns cache policy: developers may accept the flake config, while
    // self-hosted CI ignores its restricted cache settings. Pin only the CA
    // feature here so nested evaluator processes remain self-contained.
    // --result-format json --result-file emits one record per attr per phase
    // into the cwd; blast-radius consumes this on a later PR via `--timings`,
    // and check.yml uploads it as an artifact. --fail-fast stops scheduling
    // new checks as soon as one fails (in-flight builds still finish);
    // default nix-fast-build behavior would spend the full wall time before
    // flake-check goes red (#2128). The failed-attr log replay below still
    // works: the result file is written on failure with the records
    // collected so far. --eval-workers 16 with --eval-max-memory-size 6144
    // is a headroom guard rail (above nix-eval-jobs' 4 GiB default per
    // worker, below the old 8 GiB); the per-crate check split keeps each
    // worker's eval bounded by the largest single crate. The eval cache is
    // disabled: all workers share one per-flake SQLite database, so writes
    // contend ("database is busy") without providing hits on a fresh commit.
    let status = Command::new(&tools.fast_build)
        .args([
            "--flake",
            flake,
            // Drive nix-fast-build with the daemon-family-compatible
            // evaluator rather than its nixpkgs default.
            "--nix-eval-jobs",
            &tools.eval_jobs,
            "--eval-max-memory-size",
            "6144",
            "--eval-workers",
            "16",
            "--skip-cached",
            "--fail-fast",
            "--no-nom",
            "--no-link",
            "--result-format",
            "json",
            "--result-file",
            RESULT_FILE,
            "--option",
            "eval-cache",
            "false",
            "--option",
            "extra-experimental-features",
            "ca-derivations",
        ])
        .status()
        .with_context(|| format!("run {}", tools.fast_build))?;
    let build_failed = !status.success();

    if Path::new(RESULT_FILE).exists() {
        replay_failures(flake)?;
    }

    Ok(if build_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// nix-fast-build prints "Cannot build <drv>" for a failed check but not the
/// build's own output, so a clippy lint or a test panic surfaces only as a
/// bare "build exited with 1" with no diagnostic to act on. Replay each
/// failed build's log via `nix log` so the actual clippy/test output lands
/// in the CI log, then emit one `::error::` annotation per failed attr
/// (EVAL and BUILD): check.yml cats this log to the step's stdout on
/// failure, where the runner parses `::error::` lines into check-run
/// annotations -- the only failure surface reachable when raw log downloads
/// are blocked (annotations ride the checks API). Harmless plain text in a
/// local run.
fn replay_failures(flake: &str) -> Result<()> {
    let raw = fs::read_to_string(RESULT_FILE).with_context(|| format!("read {RESULT_FILE}"))?;
    let records: ResultFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {RESULT_FILE}"))?;

    for failed in records
        .results
        .iter()
        .filter(|record| record.phase == "BUILD" && !record.success)
    {
        // GitHub Actions log group so a long clippy dump stays collapsible;
        // harmless plain text in a local `nix run .#check`.
        eprintln!("::group::build log: {}", failed.attr);
        let installable = format!("{flake}.{}", failed.attr);

        // Fast path: replay the retained build log via `nix log` (works for
        // input-addressed checks like the browser smoke test).
        let drv = capture(Command::new("nix").args([
            "eval",
            "--raw",
            "--option",
            "extra-experimental-features",
            "ca-derivations",
            &format!("{installable}.drvPath"),
        ]))?;
        let drv_path = drv.stdout.trim();
        let logged = if drv.success && !drv_path.is_empty() {
            Some(capture(Command::new("nix").args(["log", drv_path]))?)
        } else {
            None
        };

        match logged {
            Some(log) if log.success && !log.stdout.trim().is_empty() => {
                eprint!("{}", log.stdout);
                // The tail as an annotation too: raw log downloads are blocked
                // from automation, and the checks API only carries annotations.
                let lines: Vec<&str> = log.stdout.lines().collect();
                println!(
                    "::error title={} build log tail::{}",
                    failed.attr,
                    tail_annotation(&lines, 10)
                );
            }
            _ => {
                // A content-addressed build (the rust units default to CA)
                // keeps its log under the *resolved* drv, which `nix log`
                // cannot fetch by the original -- so re-run the one failed
                // check with -L to stream the diagnostic (clippy lint / test
                // output). nix does not cache failures, so this just
                // re-attempts that single check.
                let rebuilt = capture(Command::new("nix").args([
                    "build",
                    &installable,
                    "-L",
                    "--no-link",
                    "--option",
                    "extra-experimental-features",
                    "ca-derivations",
                ]))?;
                eprint!("{}", rebuilt.stdout);
                eprint!("{}", rebuilt.stderr);
                let combined = format!("{}\n{}", rebuilt.stdout, rebuilt.stderr);
                let lines: Vec<&str> = combined
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect();
                println!(
                    "::error title={} build log tail::{}",
                    failed.attr,
                    tail_annotation(&lines, 10)
                );
            }
        }
        eprintln!("::endgroup::");
    }

    for failed in records.results.iter().filter(|record| !record.success) {
        let error = truncate_chars(
            &failed.error.as_deref().unwrap_or_default().replace('\n', " | "),
            ANNOTATION_ERROR_CHARS,
        );
        println!("::error title={} {}::{error}", failed.attr, failed.phase);
    }
    Ok(())
}

/// Step 2: the schema/eval gate over the package outputs. Runs
/// nix-eval-jobs over `.#packages.x86_64-linux` with its stdout teed to the
/// terminal and a report file; the gate is the `"error":` line scan (the
/// evaluator exits 0 on per-attribute failures). The report is left in
/// place on failure for inspection and removed on success.
fn schema_eval_gate(tools: &Tools) -> Result<ExitCode> {
    let tmp = tempfile::Builder::new()
        .prefix("ix-check.")
        .tempdir()
        .context("create eval report tempdir")?;
    let report_path = tmp.path().join("flake-schema-eval.jsonl");
    let gc_roots = tmp.path().join("flake-schema-eval-gc");

    let mut child = Command::new(&tools.eval_jobs)
        .args([
            "--flake",
            ".#packages.x86_64-linux",
            "--workers",
            "16",
            "--gc-roots-dir",
        ])
        .arg(&gc_roots)
        .args([
            "--option",
            "eval-cache",
            "false",
            // See the ca-derivations note in build_gate: the package set also
            // resolves content-addressed rust units, so this eval needs the
            // feature too.
            "--option",
            "extra-experimental-features",
            "ca-derivations",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {}", tools.eval_jobs))?;

    let mut report = File::create(&report_path)
        .with_context(|| format!("create {}", report_path.display()))?;
    let stdout = child.stdout.take().context("open nix-eval-jobs stdout")?;
    let mut saw_error_line = false;
    for line in BufReader::new(stdout).lines() {
        let line = line.context("read nix-eval-jobs output")?;
        println!("{line}");
        writeln!(report, "{line}").context("write eval report")?;
        if line.contains("\"error\":") {
            saw_error_line = true;
        }
    }
    let status = child.wait().context("wait for nix-eval-jobs")?;

    if !status.success() {
        // A startup/lock failure: keep the partial report around, mirror the
        // evaluator's failure.
        drop(tmp.keep());
        anyhow::bail!("{} exited with {status}", tools.eval_jobs);
    }
    if saw_error_line {
        eprintln!("flake schema evaluation failed; see the error lines above");
        drop(tmp.keep());
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

fn run() -> Result<ExitCode> {
    let args = Args::parse();
    let tools = Tools::from_env()?;
    match args.command {
        Some(Cmd::Closure) => build_gate(&tools, CLOSURE_FLAKE),
        None => {
            let gate = build_gate(&tools, CI_CHECKS_FLAKE)?;
            if gate != ExitCode::SUCCESS {
                return Ok(gate);
            }
            schema_eval_gate(&tools)
        }
    }
}

// clone:ignore -- the repo-idiomatic anyhow entry point (run, report the
// error, exit nonzero); every CLI here spells it the same way.
fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("check: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_file_records_parse() {
        let parsed: ResultFile = serde_json::from_str(
            r#"{"results": [
                {"attr": "rust-lint.clippy", "type": "BUILD", "duration": 1.0, "success": false, "error": "boom\nline2"},
                {"attr": "eval-only", "type": "EVAL", "duration": 0.1, "success": true}
            ]}"#,
        )
        .expect("result file fixture parses");
        assert_eq!(parsed.results.len(), 2);
        assert_eq!(parsed.results[0].phase, "BUILD");
        assert!(!parsed.results[0].success);
        assert_eq!(parsed.results[0].error.as_deref(), Some("boom\nline2"));
    }

    #[test]
    fn tail_annotation_joins_last_lines_and_truncates() {
        let lines: Vec<String> = (0..15).map(|n| format!("line{n}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let tail = tail_annotation(&refs, 10);
        assert!(tail.starts_with("line5 | line6"));
        assert!(tail.ends_with("line14"));

        let long = ["x".repeat(2000)];
        let refs: Vec<&str> = long.iter().map(String::as_str).collect();
        assert_eq!(tail_annotation(&refs, 10).chars().count(), ANNOTATION_TAIL_CHARS);
    }

    #[test]
    fn error_annotation_flattens_newlines() {
        let error = "boom\nline2".replace('\n', " | ");
        assert_eq!(error, "boom | line2");
        assert_eq!(truncate_chars(&error, 4), "boom");
    }
}
