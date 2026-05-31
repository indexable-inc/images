//! `indexbench` CLI.
//!
//! Subcommands:
//!
//! - `run`: execute a suite (the built-in `self-demo`, or an ad-hoc macro
//!   command via `--cmd`), record each run to the history store, and compare it
//!   against its baseline. Exits non-zero on any regression — the CI gate.
//! - `history`: list recorded runs for a `(suite, bench)`.
//! - `viewer`: stubbed HTML time-series viewer (documented fast-follow).
//!
//! The library owns all behavior; this file is the thin clap front end plus
//! output. Errors from the library are printed and mapped to an exit code in
//! `main`, never `unwrap`/`expect`ed.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use indexbench::Result;
use indexbench::compare::{CompareConfig, compare};
use indexbench::report::human_table;
use indexbench::run::{GitContext, execute};
use indexbench::store::{GitBranchStore, HistoryStore, LocalDirStore};
use indexbench::suite::{BenchSuite, MacroBench, MicroBench};

/// Metric-centric continuous benchmarking: micro + macro harnesses, durable
/// history, and a statistical regression gate.
#[derive(Debug, Parser)]
#[command(name = "indexbench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Where runs are recorded and read back from.
#[derive(Debug, Clone, ValueEnum)]
enum StoreKind {
    /// Orphan `bench-history` git branch in the repo (default; shared and
    /// versioned).
    Git,
    /// A local directory holding `history.jsonl` (laptop iteration, tests).
    Local,
}

/// Which regressions fail the gate (the process exit code).
#[derive(Debug, Clone, Copy, ValueEnum)]
enum GateKind {
    /// Any regression fails (perf job: timing, RSS, and deterministic metrics).
    All,
    /// Only deterministic-metric regressions fail. Reproducible, so this is the
    /// gate a `nix flake check` uses — sandbox-noisy timing/RSS cannot fail it.
    Deterministic,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a suite, record each run, and gate on regressions.
    Run(RunArgs),
    /// List recorded runs for a `(suite, bench)`.
    History(HistoryArgs),
    /// Stub for the HTML time-series viewer (documented fast-follow).
    Viewer(ViewerArgs),
}

#[derive(Debug, clap::Args)]
struct StoreArgs {
    /// Backing store for run history.
    #[arg(long, value_enum, default_value = "git", global = true)]
    store: StoreKind,
    /// Repository root for the git store, or the directory for the local store.
    #[arg(long, value_name = "PATH", default_value = ".", global = true)]
    repo: PathBuf,
    /// Branch name for the git store.
    #[arg(long, default_value = GitBranchStore::DEFAULT_BRANCH, global = true)]
    branch: String,
    /// Directory for the local store.
    #[arg(long, value_name = "PATH", default_value = ".indexbench", global = true)]
    local_dir: PathBuf,
}

impl StoreArgs {
    /// Construct the selected store.
    fn open(&self) -> Result<Box<dyn HistoryStore>> {
        match self.store {
            StoreKind::Git => Ok(Box::new(GitBranchStore::new(self.repo.clone(), self.branch.clone()))),
            StoreKind::Local => Ok(Box::new(LocalDirStore::new(&self.local_dir)?)),
        }
    }
}

#[derive(Debug, clap::Args)]
struct RunArgs {
    /// Suite to run. `self-demo` is the built-in proof-of-loop suite; any other
    /// name requires `--cmd` to define the bench.
    #[arg(long, default_value = "self-demo")]
    suite: String,
    /// Ad-hoc macro bench: the command to run, e.g. `--cmd "sleep 0.01"`. The
    /// first token is the program; the rest are arguments. Repeat to add benches.
    #[arg(long, value_name = "COMMAND")]
    cmd: Vec<String>,
    /// Bench name for an ad-hoc `--cmd`. Defaults to the program name.
    #[arg(long, value_name = "NAME")]
    cmd_name: Option<String>,
    /// How many times to run each macro command.
    #[arg(long, default_value_t = 5)]
    runs: u32,
    /// Compare against this commit's recorded run instead of the previous run.
    #[arg(long, value_name = "COMMIT")]
    baseline: Option<String>,
    /// Relative effect-size threshold (fraction, e.g. 0.02 for 2%).
    #[arg(long, default_value_t = indexbench::compare::DEFAULT_THRESHOLD)]
    threshold: f64,
    /// Significance level for the distributional test.
    #[arg(long, default_value_t = indexbench::compare::DEFAULT_ALPHA)]
    alpha: f64,
    /// Which regressions fail the gate. `all` for the perf job; `deterministic`
    /// for a reproducible flake check.
    #[arg(long, value_enum, default_value = "all")]
    gate: GateKind,
    /// Emit JSON instead of the human table.
    #[arg(long)]
    output_json: bool,
    #[command(flatten)]
    store: StoreArgs,
}

#[derive(Debug, clap::Args)]
struct HistoryArgs {
    /// Suite to list.
    #[arg(long)]
    suite: String,
    /// Bench to list.
    #[arg(long)]
    bench: String,
    #[command(flatten)]
    store: StoreArgs,
}

#[derive(Debug, clap::Args)]
struct ViewerArgs {
    #[command(flatten)]
    store: StoreArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("indexbench: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Dispatch a parsed CLI, returning the process exit code. A regression in any
/// compared bench yields `ExitCode::FAILURE` so CI gates on it.
fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Run(args) => run_suite(&args),
        Command::History(args) => show_history(&args),
        Command::Viewer(args) => Ok(show_viewer(&args)),
    }
}

/// Build the requested suite, execute it, record each run, and compare.
fn run_suite(args: &RunArgs) -> Result<ExitCode> {
    let store = args.store.open()?;
    let git = GitContext::resolve(&args.store.repo);

    let mut suite = build_suite(args);
    let runs = execute(&mut suite, &git)?;

    let config = CompareConfig {
        alpha: args.alpha,
        threshold: args.threshold,
    };

    let mut any_regression = false;
    let mut json_comparisons = Vec::new();

    for run in &runs {
        // Record before comparing so the baseline lookup for the *next* run sees
        // this one, and so a comparison failure cannot lose the measurement.
        store.append(run)?;

        let baseline = match &args.baseline {
            Some(commit) => store.run_at_commit(&run.suite, &run.bench, &run.machine_id, commit)?,
            None => baseline_excluding_self(store.as_ref(), run)?,
        };

        match baseline {
            Some(baseline) => {
                let comparison = compare(&baseline, run, config);
                any_regression |= match args.gate {
                    GateKind::All => comparison.has_regression(),
                    GateKind::Deterministic => comparison.has_deterministic_regression(),
                };
                if args.output_json {
                    json_comparisons.push(comparison);
                } else {
                    if run.git_dirty {
                        println!("(working tree dirty; baseline comparison may be noisy)");
                    }
                    print!("{}", human_table(&comparison));
                    println!();
                }
            }
            None => {
                if args.output_json {
                    // No comparison yet; emit the recorded run so a first-run
                    // JSON consumer still gets the measurement.
                    json_comparisons.push(first_run_placeholder(run));
                } else {
                    println!("{}/{}: recorded baseline (no prior run to compare)\n", run.suite, run.bench);
                }
            }
        }
    }

    if args.output_json {
        let rendered = serde_json::to_string_pretty(&json_comparisons).map_err(|source| indexbench::Error::Serialize { source })?;
        println!("{rendered}");
    }

    Ok(if any_regression { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// The previous recorded run for this bench on this machine, excluding the run
/// just appended. The store keeps insertion order within a timestamp, so the
/// baseline is the most recent same-machine run that is not the current one;
/// dropping the last entry equal to `current` handles the common case where a
/// rapid re-run shares the whole-second timestamp. Without this the freshly
/// recorded run would be its own baseline and every comparison a no-op.
fn baseline_excluding_self(store: &dyn HistoryStore, current: &indexbench::Run) -> Result<Option<indexbench::Run>> {
    let mut runs = store.runs_for(&current.suite, &current.bench)?;
    runs.retain(|candidate| candidate.machine_id == current.machine_id);

    // Drop the single most-recent entry identical to `current` (the row just
    // appended), then take the last of what remains as the baseline.
    if let Some(position) = runs.iter().rposition(|candidate| candidate == current) {
        runs.remove(position);
    }
    Ok(runs.pop())
}

/// A JSON-friendly placeholder for a first run with no baseline yet.
fn first_run_placeholder(run: &indexbench::Run) -> indexbench::compare::Comparison {
    indexbench::compare::Comparison {
        suite: run.suite.clone(),
        bench: run.bench.clone(),
        metrics: Vec::new(),
    }
}

/// Build the suite the run was asked for: the built-in `self-demo` micro+macro
/// pair when no `--cmd` is given, plus any ad-hoc macro commands.
fn build_suite(args: &RunArgs) -> BenchSuite<'static> {
    let mut suite = BenchSuite::new(args.suite.clone());

    if args.cmd.is_empty() && args.suite == "self-demo" {
        // The proof-of-loop suite: one micro Rust fn and one trivial macro
        // command, enough to exercise run -> record -> compare end to end.
        suite = suite
            .micro(MicroBench::new("fib", || {
                std::hint::black_box(fib(std::hint::black_box(20)));
            }))
            .macro_bench(MacroBench::new("true", "true", Vec::<String>::new()).with_runs(args.runs));
    }

    for command in &args.cmd {
        let mut parts = command.split_whitespace();
        let program = parts.next().unwrap_or("true").to_owned();
        let cmd_args: Vec<String> = parts.map(str::to_owned).collect();
        let name = args.cmd_name.clone().unwrap_or_else(|| program.clone());
        suite = suite.macro_bench(MacroBench::new(name, program, cmd_args).with_runs(args.runs));
    }

    suite
}

/// A small CPU-bound function for the self-demo micro bench.
fn fib(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fib(n - 1) + fib(n - 2),
    }
}

/// List recorded runs for a `(suite, bench)`.
fn show_history(args: &HistoryArgs) -> Result<ExitCode> {
    let store = args.store.open()?;
    let runs = store.runs_for(&args.suite, &args.bench)?;
    if runs.is_empty() {
        println!("no runs recorded for {}/{}", args.suite, args.bench);
        return Ok(ExitCode::SUCCESS);
    }
    for run in &runs {
        let metrics: Vec<String> = run.metrics.iter().map(|m| format!("{}={:.3}{}", m.name, m.value, m.unit)).collect();
        println!(
            "{ts}  {commit}{dirty}  {machine}  {metrics}",
            ts = run.timestamp_unix,
            commit = short_commit(&run.git_commit),
            dirty = if run.git_dirty { "*" } else { "" },
            machine = run.machine_id,
            metrics = metrics.join("  "),
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// First 12 chars of a commit, or the whole string when shorter.
fn short_commit(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Stub for the time-series viewer (a documented fast-follow). Takes the store
/// args for signature symmetry with the other subcommands even though the stub
/// does not read them yet.
fn show_viewer(_args: &ViewerArgs) -> ExitCode {
    println!("indexbench viewer is a documented fast-follow.");
    println!("Planned: render the JSONL history as an interactive HTML time-series");
    println!("per (machine, suite, bench, metric), served on a local port.");
    println!("For now, inspect history with `indexbench history --suite <s> --bench <b>`");
    println!("or read the JSONL store directly.");
    ExitCode::SUCCESS
}
