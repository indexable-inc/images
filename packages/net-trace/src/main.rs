//! `net-trace`: run a command under a recording localhost proxy and report
//! the client-side network it touched. See `lib.rs` for the model and
//! `.github/scripts/run-required-gate.sh` for the CI wiring (#4031).

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, Result, bail};
use net_trace::proxy::{self, Recorder};
use net_trace::report::{self, Phase};

#[derive(Parser)]
#[command(about = "Record the client-side network activity of a wrapped command")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a command under the proxy and write `<dir>/<label>.json`.
    Run {
        /// Phase label: kebab-case; becomes the file name and a report key.
        #[arg(long)]
        label: String,
        /// Directory phase files accumulate in across `run` invocations.
        #[arg(long)]
        dir: PathBuf,
        /// The command, after `--`.
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Fold every phase file in `--dir` into a report on stdout.
    Render {
        #[arg(long)]
        dir: PathBuf,
        /// Emit the constrained summary JSON consumed by the trusted comment
        /// job instead of Markdown.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    color_eyre::install()?;
    match Cli::parse().command {
        Command::Run { label, dir, cmd } => {
            let code = run(&label, &dir, &cmd)?;
            std::process::exit(code);
        }
        Command::Render { dir, json } => {
            let summary = report::summarize(&report::load(&dir)?);
            if json {
                println!("{}", serde_json::to_string(&summary).wrap_err("serialize summary")?);
            } else {
                print!("{}", report::markdown(&summary));
            }
            Ok(())
        }
    }
}

/// Runs the child with every common proxy variable pointed at the recorder,
/// writes the phase file, and returns the child's exit code (1 for a
/// signal death) so the wrapper is exit-transparent.
fn run(label: &str, dir: &std::path::Path, cmd: &[String]) -> Result<i32> {
    validate_label(label)?;
    let recorder = Arc::new(Recorder::new());
    let port = proxy::spawn(Arc::clone(&recorder))?;
    let proxy_url = format!("http://127.0.0.1:{port}");

    let started_at_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .wrap_err("system clock before epoch")?
            .as_millis(),
    )
    .expect("epoch millis fit u64");
    let started = Instant::now();
    let (program, args) = cmd.split_first().expect("clap enforces a non-empty command");
    let mut child = Process::new(program);
    child.args(args);
    // Lowercase and uppercase both: curl reads either, git reads lowercase,
    // some tools only the uppercase forms.
    for variable in ["http_proxy", "https_proxy", "all_proxy"] {
        child.env(variable, &proxy_url);
        child.env(variable.to_uppercase(), &proxy_url);
    }
    // An inherited no_proxy would let matching hosts bypass the recorder.
    child.env_remove("no_proxy");
    child.env_remove("NO_PROXY");
    let status =
        child.status().wrap_err_with(|| format!("spawn {program}"))?;

    // The child's sockets EOF our tunnels at exit; give handler threads a
    // beat to push their records before snapshotting.
    thread::sleep(Duration::from_millis(300));
    let phase = Phase {
        label: label.to_owned(),
        cmd: cmd.to_vec(),
        started_at_ms,
        wall_ms: u64::try_from(started.elapsed().as_millis()).expect("wall millis fit u64"),
        exit_code: status.code(),
        connections: recorder.snapshot(),
    };
    std::fs::create_dir_all(dir).wrap_err_with(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{label}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(&phase).wrap_err("serialize phase")?)
        .wrap_err_with(|| format!("write {}", path.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Labels become file names and, downstream, jq-gated report keys; keep them
/// kebab-case so a caller cannot smuggle a path or a hostile key.
fn validate_label(label: &str) -> Result<()> {
    let mut chars = label.chars();
    let starts_lower = chars.next().is_some_and(|first| first.is_ascii_lowercase());
    if !starts_lower || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        bail!("label must match ^[a-z][a-z0-9-]*$: {label}");
    }
    Ok(())
}
