//! Deploy nix-darwin configurations to remote macOS hosts, colmena-style:
//! build each system closure locally, `nix copy` it to its host over ssh,
//! then set the system profile and run the nix-darwin activation scripts.

mod deploy;
mod exec;
mod node;
mod plan;
mod report;

use std::process::ExitCode;
use std::thread;

use anyhow::Result;
use clap::Parser;

use crate::node::NodeSpec;
use crate::report::{NodeReport, Report};

/// Deploy `darwinConfigurations.<name>` from a flake to remote macOS hosts.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Flake ref holding `darwinConfigurations` (a path, `github:owner/repo`, ...).
    #[arg(long, default_value = ".")]
    flake: String,

    /// Build and compare against each host's current system; skip copy and
    /// activation.
    #[arg(long)]
    dry_run: bool,

    /// Emit one JSON report document on stdout instead of a human summary.
    #[arg(long)]
    json: bool,

    /// Nodes to deploy, each `<name>=<[user@]host>`: `<name>` indexes
    /// `darwinConfigurations`, the right side is the ssh destination.
    #[arg(required = true, value_name = "NAME=[USER@]HOST")]
    nodes: Vec<NodeSpec>,
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();

    let nodes: Vec<NodeReport> = thread::scope(|scope| {
        // Collect all spawns before the first join: folding this into one
        // iterator chain (clippy's suggestion) would join each thread before
        // spawning the next, serializing the deploys.
        #[allow(clippy::needless_collect)]
        let handles: Vec<_> = cli
            .nodes
            .iter()
            .map(|spec| scope.spawn(|| deploy::node(&cli.flake, spec, cli.dry_run)))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("deploy thread panicked"))
            .collect()
    });

    let report = Report::new(nodes, cli.dry_run);
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print_human();
    }
    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
