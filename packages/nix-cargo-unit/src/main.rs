mod hash;
mod model;
mod render;
mod shell;

use std::io::Read as _;
use std::path::PathBuf;

use clap::Parser as _;
use color_eyre::eyre::WrapErr as _;
use model::UnitGraph;
use render::{CargoLockSources, RenderOptions, render_units_nix};

#[derive(Debug, clap::Parser)]
#[command(
    version,
    about = "Render Cargo unit graphs as composable Nix derivations"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Merge several Cargo unit-graph JSON files.
    Merge(MergeArgs),

    /// Render generated Nix from Cargo unit-graph JSON on stdin.
    Render(RenderArgs),
}

#[derive(Debug, clap::Args)]
struct MergeArgs {
    /// Cargo unit-graph JSON files to merge.
    #[arg(required = true, value_name = "PATH")]
    graphs: Vec<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct RenderArgs {
    /// Canonical workspace root from cargo --unit-graph.
    #[arg(long, default_value = ".", value_name = "PATH")]
    workspace_root: PathBuf,

    /// Cargo vendor directory used for registry/git crates.
    #[arg(long, value_name = "PATH")]
    vendor_root: Option<PathBuf>,

    /// Cargo.lock used to resolve exact registry, sparse, and git source identities.
    #[arg(long, value_name = "PATH")]
    cargo_lock: PathBuf,

    /// Emit CA-derivation attributes on generated units.
    #[arg(long)]
    content_addressed: bool,

    /// Salt unit identity hashes with a Rust toolchain id.
    #[arg(long, value_name = "ID")]
    toolchain_id: Option<String>,

    /// Collect and fail builds on dependencies unused across all local package units.
    #[arg(long)]
    deny_unused_crate_dependencies: bool,
}

fn merge(args: MergeArgs) -> color_eyre::Result<()> {
    let graphs = args
        .graphs
        .into_iter()
        .map(|path| {
            let input = std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("reading Cargo unit graph {}", path.display()))?;
            let graph: UnitGraph = serde_json::from_str(&input)
                .wrap_err_with(|| format!("parsing Cargo unit graph {}", path.display()))?;
            Ok(graph)
        })
        .collect::<color_eyre::Result<Vec<_>>>()?;

    let merged = UnitGraph::merge(graphs).wrap_err("merging Cargo unit graphs")?;
    serde_json::to_writer(std::io::stdout(), &merged)
        .wrap_err("writing merged Cargo unit graph")?;
    println!();

    Ok(())
}

fn render(args: RenderArgs) -> color_eyre::Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .wrap_err("reading Cargo unit graph from stdin")?;
    let graph: UnitGraph =
        serde_json::from_str(&input).wrap_err("parsing Cargo unit graph JSON")?;
    let cargo_lock_sources = CargoLockSources::from_path(&args.cargo_lock)?;

    let rendered = render_units_nix(
        &graph,
        &RenderOptions {
            workspace_root: args.workspace_root,
            vendor_root: args.vendor_root,
            cargo_lock_sources,
            content_addressed: args.content_addressed,
            toolchain_id: args.toolchain_id,
            deny_unused_crate_dependencies: args.deny_unused_crate_dependencies,
        },
    )
    .wrap_err("rendering Cargo unit graph as Nix")?;
    print!("{rendered}");

    Ok(())
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    match Cli::parse().command {
        Command::Merge(args) => merge(args),
        Command::Render(args) => render(args),
    }
}
