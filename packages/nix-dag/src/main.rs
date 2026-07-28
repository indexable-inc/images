//! `nix-dag`: score the shape of a Nix build plan.
//!
//! Answers three questions about a plan without building it: how long it must
//! take at any width (the critical path), how much parallelism is on offer
//! (the width profile), and which derivations are dragging the rest of the
//! graph into a rebuild they get nothing from (the carrier ranking).
//!
//! The last one is the reason this exists. An environment variable holding a
//! store path is a dependency edge with none of the visibility: it does not
//! appear in `buildInputs`, nothing in the build reads it, and every derivation
//! carrying it re-hashes whenever the thing it names moves.

mod graph;
mod plan;
mod report;

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use color_eyre::eyre::{Result, bail};

use graph::Metrics;
use plan::Plan;
use report::Report;

#[derive(Parser)]
#[command(
    about = "Score a Nix build plan: critical path, parallelism, and which nodes invalidate the rest for nothing"
)]
struct Cli {
    /// Flake installable or `.drv` path to read the plan from, e.g.
    /// `.#required-ci-checks`. Evaluation only; nothing is built.
    installable: Option<String>,
    /// Read a captured `nix derivation show --recursive` dump instead of
    /// evaluating. `-` reads stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "installable")]
    from_json: Option<PathBuf>,
    /// How many derivations to rank.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Emit the report as JSON.
    #[arg(long)]
    json: bool,
}

/// Phase labels are stable: they show up in CI logs and in anything reading the
/// stderr trace, so renaming one breaks a downstream reader.
fn phase(label: &str, since: Instant) -> Instant {
    eprintln!("nix-dag: {label}: {:.2}s", since.elapsed().as_secs_f64());
    Instant::now()
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    let mut clock = Instant::now();
    let (target, plan) = match (&cli.installable, &cli.from_json) {
        (Some(installable), _) => (installable.clone(), Plan::from_installable(installable)?),
        (None, Some(path)) => (path.display().to_string(), Plan::from_file(path)?),
        (None, None) => bail!("give an installable to evaluate, or --from-json to read a dump"),
    };
    clock = phase("load", clock);

    let metrics = Metrics::compute(&plan)?;
    phase("metrics", clock);

    let report = Report::build(&target, &plan, &metrics, cli.top);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{report}");
    }
    Ok(())
}
