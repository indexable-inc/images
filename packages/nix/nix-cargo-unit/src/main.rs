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

    /// Emit cargo-nextest metadata for one cargo-unit-built test binary.
    NextestMetadata(NextestMetadataArgs),

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
struct NextestMetadataArgs {
    /// Synthetic workspace root nextest should report in diagnostics.
    #[arg(long, value_name = "PATH")]
    workspace_root: PathBuf,

    /// Cargo-unit test target name.
    #[arg(long, value_name = "NAME")]
    target_name: String,

    /// Cargo package name that owns the test target.
    #[arg(long, value_name = "NAME")]
    package_name: String,

    /// Rust edition from the Cargo package target.
    #[arg(long, value_name = "EDITION")]
    edition: String,

    /// Cargo-unit-built libtest binary to run through nextest.
    #[arg(long, value_name = "PATH")]
    test_binary: PathBuf,

    /// Rust target triple used to build the test binary.
    #[arg(long, value_name = "TRIPLE")]
    target_triple: String,

    /// Rust target libdir for nextest build metadata.
    #[arg(long, value_name = "PATH")]
    rust_libdir: PathBuf,

    /// Output path for cargo metadata JSON.
    #[arg(long, value_name = "PATH")]
    cargo_metadata: PathBuf,

    /// Output path for nextest binaries metadata JSON.
    #[arg(long, value_name = "PATH")]
    binaries_metadata: PathBuf,
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

fn nextest_metadata(args: &NextestMetadataArgs) -> color_eyre::Result<()> {
    let target_directory = args.workspace_root.join("target").display().to_string();
    let package_id = format!(
        "path+file://{}#{}@0.0.0",
        args.workspace_root.display(),
        args.target_name
    );

    let cargo_metadata = nextest_cargo_metadata(args, &package_id, &target_directory);
    let binaries_metadata = nextest_binaries_metadata(args, &package_id, &target_directory);

    write_json(&args.cargo_metadata, &cargo_metadata)?;
    write_json(&args.binaries_metadata, &binaries_metadata)?;

    Ok(())
}

fn nextest_cargo_metadata(
    args: &NextestMetadataArgs,
    package_id: &str,
    target_directory: &str,
) -> serde_json::Value {
    let manifest_path = args.workspace_root.join("Cargo.toml").display().to_string();
    let src_path = args.workspace_root.join("src/lib.rs").display().to_string();

    let cargo_target = serde_json::json!({
        "kind": ["lib"],
        "crate_types": ["lib"],
        "name": &args.package_name,
        "src_path": src_path,
        "edition": &args.edition,
        "doc": true,
        "doctest": false,
        "test": true
    });
    let cargo_package = serde_json::json!({
        "name": &args.package_name,
        "version": "0.0.0",
        "id": package_id,
        "source": null,
        "dependencies": [],
        "features": {},
        "manifest_path": manifest_path,
        "edition": &args.edition,
        "metadata": null,
        "publish": null,
        "authors": [],
        "categories": [],
        "keywords": [],
        "license": null,
        "license_file": null,
        "description": null,
        "readme": null,
        "repository": null,
        "homepage": null,
        "documentation": null,
        "links": null,
        "default_run": null,
        "rust_version": null,
        "targets": [cargo_target]
    });

    serde_json::json!({
        "version": 1,
        "workspace_root": args.workspace_root.display().to_string(),
        "target_directory": target_directory,
        "workspace_members": [package_id],
        "workspace_default_members": [package_id],
        "resolve": null,
        "metadata": null,
        "packages": [cargo_package]
    })
}

fn nextest_binaries_metadata(
    args: &NextestMetadataArgs,
    package_id: &str,
    target_directory: &str,
) -> serde_json::Value {
    let rust_libdir = args.rust_libdir.display().to_string();
    let test_binary = args.test_binary.display().to_string();

    let host_platform = serde_json::json!({
        "platform": {
            "triple": &args.target_triple,
            "target-features": "unknown"
        },
        "libdir": {
            "status": "available",
            "path": rust_libdir
        }
    });
    let build_meta = serde_json::json!({
        "target-directory": target_directory,
        "build-directory": target_directory,
        "base-output-directories": ["debug"],
        "non-test-binaries": {},
        "build-script-out-dirs": {},
        "build-script-info": {},
        "linked-paths": [],
        "platforms": {
            "host": host_platform,
            "targets": []
        },
        "target-platforms": [
            {
                "triple": &args.target_triple,
                "target-features": "unknown"
            }
        ],
        "target-platform": null
    });
    let binary = serde_json::json!({
        "binary-id": &args.target_name,
        "binary-name": &args.target_name,
        "package-id": package_id,
        "kind": "lib",
        "binary-path": test_binary,
        "build-platform": "target"
    });

    serde_json::json!({
        "rust-build-meta": build_meta,
        "rust-binaries": {
            args.target_name.as_str(): binary
        }
    })
}

fn write_json(path: &std::path::Path, value: &serde_json::Value) -> color_eyre::Result<()> {
    let file = std::fs::File::create(path)
        .wrap_err_with(|| format!("creating JSON output {}", path.display()))?;
    serde_json::to_writer_pretty(file, value)
        .wrap_err_with(|| format!("writing JSON output {}", path.display()))
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
        Command::NextestMetadata(args) => nextest_metadata(&args),
        Command::Render(args) => render(args),
        Command::ScanPanics(args) => scan_panics(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nextest_metadata_writes_package_and_binary_metadata() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let workspace_root = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("create workspace root");
        let cargo_metadata = tmp.path().join("cargo-metadata.json");
        let binaries_metadata = tmp.path().join("binaries-metadata.json");

        nextest_metadata(&NextestMetadataArgs {
            workspace_root: workspace_root.clone(),
            target_name: "crate_tests".to_owned(),
            package_name: "crate-name".to_owned(),
            edition: "2024".to_owned(),
            test_binary: PathBuf::from("/nix/store/test-binary/bin/crate_tests"),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            rust_libdir: PathBuf::from("/nix/store/rust/lib/rustlib/x86_64-unknown-linux-gnu/lib"),
            cargo_metadata: cargo_metadata.clone(),
            binaries_metadata: binaries_metadata.clone(),
        })
        .expect("write nextest metadata");

        let cargo: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(cargo_metadata).expect("read cargo metadata"),
        )
        .expect("parse cargo metadata");
        let binaries: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(binaries_metadata).expect("read binaries metadata"),
        )
        .expect("parse binaries metadata");

        assert_eq!(cargo["packages"][0]["name"], "crate-name");
        assert_eq!(cargo["packages"][0]["edition"], "2024");
        assert_eq!(cargo["packages"][0]["targets"][0]["edition"], "2024");
        assert_eq!(
            cargo["packages"][0]["manifest_path"],
            workspace_root.join("Cargo.toml").display().to_string()
        );
        assert_eq!(
            binaries["rust-binaries"]["crate_tests"]["binary-path"],
            "/nix/store/test-binary/bin/crate_tests"
        );
        assert_eq!(
            binaries["rust-build-meta"]["platforms"]["host"]["platform"]["triple"],
            "x86_64-unknown-linux-gnu"
        );
    }
}
