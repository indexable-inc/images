//! `lint`: run every repo lint stage in parallel. The default path execs
//! dag-runner over the generated spec (live per-stage progress); `--json`
//! (#1683) runs the same spec nodes directly and emits one JSON document.
//! Both the spec path and the dag-runner path arrive via the wrapper env
//! (`IX_LINT_SPEC`, `IX_DAG_RUNNER`) baked in default.nix.

use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, ensure};

fn wrapper_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is not set; run lint via its wrapped package"))
}

fn run_json(spec_path: &str) -> Result<ExitCode> {
    let spec = lint::spec::load(spec_path)?;
    let runs = lint::spec::run_all(&spec)?;
    println!("{}", lint::spec::to_json(&runs)?);
    let worst = lint::spec::worst_exit_code(&runs);
    // A status outside u8 (e.g. signal death surfaced as -1) is still a failure.
    Ok(u8::try_from(worst).map_or(ExitCode::FAILURE, ExitCode::from))
}

fn run() -> Result<ExitCode> {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let spec_path = wrapper_env("IX_LINT_SPEC")?;

    if args.iter().any(|arg| arg == "--json") {
        ensure!(args.len() == 1, "--json takes no other arguments");
        return run_json(&spec_path);
    }

    let dag_runner = wrapper_env("IX_DAG_RUNNER")?;
    let err = Command::new(&dag_runner)
        .args(args)
        .arg(&spec_path)
        .exec();
    Err(err).with_context(|| format!("exec {dag_runner}"))
}

// clone:ignore -- the repo-idiomatic anyhow entry point (run, report the
// error, exit nonzero); every CLI here spells it the same way.
fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("lint: {err:#}");
            ExitCode::FAILURE
        }
    }
}
