mod hash;
mod model;
mod panic_scan;
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

    /// Scan compiled rlib artifacts for functions that can reach a panic.
    ScanPanics(ScanPanicsArgs),
}

#[derive(Debug, clap::Args)]
struct ScanPanicsArgs {
    /// Workspace crate (Cargo target name) whose functions findings are scoped
    /// to. Repeat for the full workspace set so a library generic monomorphized
    /// in another unit's object is still attributed. Omit to report every
    /// panic-reaching function.
    #[arg(long = "crate-name", value_name = "NAME")]
    crate_names: Vec<String>,

    /// Rlib or object artifacts, or directories to scan. Directories are
    /// searched for `*.rlib` and `*.o` recursively.
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
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

    /// Emit a per-unit panic-freedom policy check that scans each local unit's
    /// compiled artifact for reachable panic machinery and fails if any is found.
    #[arg(long)]
    deny_panics: bool,
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
            deny_panics: args.deny_panics,
        },
    )
    .wrap_err("rendering Cargo unit graph as Nix")?;
    print!("{rendered}");

    Ok(())
}

fn scan_panics(args: ScanPanicsArgs) -> color_eyre::Result<()> {
    let ScanPanicsArgs { crate_names, paths } = args;
    let artifacts = panic_scan::collect_artifacts(&paths)?;
    // Fail closed: a panic gate that finds nothing to inspect must error, not
    // report success, or a wrong path or empty object set would pass open.
    if artifacts.is_empty() {
        color_eyre::eyre::bail!(
            "cargo-unit panic-freedom: no .rlib or .o artifacts found under {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let crate_tokens: Vec<String> = crate_names
        .iter()
        .map(|name| panic_scan::crate_token(name))
        .collect();
    let findings = panic_scan::scan_paths(&artifacts, &crate_tokens)?;

    if findings.is_empty() {
        return Ok(());
    }

    let scope = if crate_names.is_empty() {
        String::new()
    } else {
        format!(" in {}", crate_names.join(", "))
    };
    eprintln!(
        "error: cargo-unit panic-freedom: {} function(s){scope} can reach panic machinery",
        findings.len()
    );
    for finding in &findings {
        eprintln!("  {} -> {}", finding.function, finding.panic_entrypoint);
    }
    std::process::exit(1);
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    match Cli::parse().command {
        Command::Merge(args) => merge(args),
        Command::Render(args) => render(args),
        Command::ScanPanics(args) => scan_panics(args),
    }
}
