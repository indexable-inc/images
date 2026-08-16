mod hash;
mod model;
mod panic_scan;
mod render;
mod shell;

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

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

    /// Emit cargo-nextest metadata for every cargo-unit-built test binary in a
    /// workspace, so one `cargo nextest run --binaries-metadata
    /// --cargo-metadata --workspace-remap <checkout>` runs the whole prebuilt
    /// suite on a machine that never compiled it.
    NextestMetadataWorkspace(NextestMetadataWorkspaceArgs),

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
struct NextestMetadataWorkspaceArgs {
    /// Synthetic workspace root recorded in the metadata; consumers remap it
    /// onto the real checkout with `cargo nextest run --workspace-remap`.
    #[arg(long, value_name = "PATH")]
    workspace_root: PathBuf,

    /// JSON list of test-binary records, each `{"target-name",
    /// "package-name", "package-version", "package-root", "kind", "edition",
    /// "binary-path"}` (the shape `units.testTargets` exports).
    #[arg(long, value_name = "PATH")]
    binaries: PathBuf,

    /// Optional JSON list of non-test binary records `{"package-id", "name",
    /// "kind", "path"}` for workspace bins that tests spawn through
    /// `CARGO_BIN_EXE_<name>`.
    #[arg(long, value_name = "PATH")]
    non_test_binaries: Option<PathBuf>,

    /// Optional JSON map of package id to that package's build-script
    /// `OUT_DIR`.
    #[arg(long, value_name = "PATH")]
    build_script_out_dirs: Option<PathBuf>,

    /// Library search path for nextest's `linked-paths` (repeatable; passed
    /// through verbatim, including any `native=`-style kind prefix cargo
    /// recorded).
    #[arg(long = "linked-path", value_name = "PATH")]
    linked_paths: Vec<String>,

    /// Rust target triple used to build the test binaries.
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

/// One prebuilt test binary in the workspace export. Kebab-case keys match
/// the JSON the nix layer synthesizes from `units.testTargets`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct WorkspaceTestBinary {
    target_name: String,
    package_name: String,
    package_version: String,
    /// Package directory relative to the workspace root (`.` or empty for a
    /// root package), exactly as `testTargets.packageRoot` records it.
    package_root: String,
    /// Cargo target kind ("lib", "test", "bin", "bench", ...); decides the
    /// nextest binary-id spelling.
    kind: String,
    edition: String,
    binary_path: String,
}

/// A workspace bin a test spawns via `CARGO_BIN_EXE_<name>`; mirrors
/// nextest's `RustNonTestBinarySummary` (kind is e.g. "bin-exe").
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct WorkspaceNonTestBinary {
    package_id: String,
    name: String,
    kind: String,
    path: String,
}

// Independent CLI switches, not a state machine (same shape as the render
// options bag it feeds).
#[allow(clippy::struct_excessive_bools)]
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

    /// Compile lib-only units with `-Zembed-metadata=no`: their rlib carries a
    /// metadata stub, and dependents pass each such crate twice, `--extern
    /// name=<rlib>` plus `--extern name=<sibling .rmeta>` (cargo's
    /// -Zno-embed-metadata scheme), so compiles read full metadata and links
    /// consume the thin rlib. Requires a nightly rustc.
    #[arg(long)]
    no_embed_metadata: bool,

    /// Extra rustc flag for metadata-emitting lib compiles (Rustc driver
    /// only; repeatable, order preserved). The policy layer pairs these with
    /// a toolchain that accepts them; the renderer just transcribes.
    #[arg(long = "rmeta-stability-flag", value_name = "FLAG")]
    rmeta_stability_flags: Vec<String>,
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

    let build_meta = nextest_build_meta(
        &args.target_triple,
        &rust_libdir,
        target_directory,
        &serde_json::json!({}),
        &BTreeMap::new(),
        &[],
    );
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

/// The `rust-build-meta` block both metadata modes share; one synthesis so the
/// single-binary and workspace outputs cannot drift apart in shape.
fn nextest_build_meta(
    target_triple: &str,
    rust_libdir: &str,
    target_directory: &str,
    non_test_binaries: &serde_json::Value,
    build_script_out_dirs: &BTreeMap<String, String>,
    linked_paths: &[String],
) -> serde_json::Value {
    serde_json::json!({
        "target-directory": target_directory,
        "build-directory": target_directory,
        "base-output-directories": ["debug"],
        "non-test-binaries": non_test_binaries,
        "build-script-out-dirs": build_script_out_dirs,
        "build-script-info": {},
        "linked-paths": linked_paths,
        "platforms": {
            "host": {
                "platform": {
                    "triple": target_triple,
                    "target-features": "unknown"
                },
                "libdir": {
                    "status": "available",
                    "path": rust_libdir
                }
            },
            "targets": []
        },
        "target-platforms": [
            {
                "triple": target_triple,
                "target-features": "unknown"
            }
        ],
        "target-platform": null
    })
}

/// Package directory a workspace-relative `packageRoot` names. `.` and empty
/// both mean the workspace root itself (`SourceEntry::package_root` emits `.`
/// for a root package).
fn workspace_package_dir(workspace_root: &Path, package_root: &str) -> PathBuf {
    if package_root.is_empty() || package_root == "." {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(package_root)
    }
}

/// The one spelling of a workspace package id. Both output files synthesize
/// ids through here, so binaries-metadata's package-ids byte-match
/// cargo-metadata's packages and `workspace_members` by construction (nextest
/// hard-fails on any mismatch).
fn workspace_package_id(dir: &Path, package_name: &str, package_version: &str) -> String {
    format!(
        "path+file://{}#{package_name}@{package_version}",
        dir.display()
    )
}

/// The binary-id spelling cargo-nextest's `RustBinaryId::from_parts` uses: a
/// library kind's unittest binary is the bare package name, an integration
/// test is `package::target`, and every other kind is
/// `package::kind/target`.
fn nextest_binary_id(package_name: &str, kind: &str, target_name: &str) -> String {
    match kind {
        "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro" => {
            package_name.to_owned()
        }
        "test" => format!("{package_name}::{target_name}"),
        _ => format!("{package_name}::{kind}/{target_name}"),
    }
}

/// One cargo package the workspace export mentions, keyed by its synthesized
/// package id.
struct WorkspacePackage {
    name: String,
    version: String,
    /// A package-level edition only decorates diagnostics (cargo editions are
    /// per-target); the first test target's edition stands in for it.
    edition: String,
    dir: PathBuf,
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> color_eyre::Result<T> {
    let input = std::fs::read_to_string(path)
        .wrap_err_with(|| format!("reading JSON input {}", path.display()))?;
    serde_json::from_str(&input).wrap_err_with(|| format!("parsing JSON input {}", path.display()))
}

fn nextest_metadata_workspace(args: &NextestMetadataWorkspaceArgs) -> color_eyre::Result<()> {
    let binaries: Vec<WorkspaceTestBinary> = read_json_file(&args.binaries)?;
    // Fail closed: an empty export would make every downstream nextest run
    // vacuously green (`--no-tests=pass` class), so refuse to synthesize one.
    if binaries.is_empty() {
        color_eyre::eyre::bail!(
            "nextest-metadata-workspace: {} lists no test binaries",
            args.binaries.display()
        );
    }
    let non_test_binaries: Vec<WorkspaceNonTestBinary> = args
        .non_test_binaries
        .as_deref()
        .map(read_json_file)
        .transpose()?
        .unwrap_or_default();
    let build_script_out_dirs: BTreeMap<String, String> = args
        .build_script_out_dirs
        .as_deref()
        .map(read_json_file)
        .transpose()?
        .unwrap_or_default();

    let mut packages: BTreeMap<String, WorkspacePackage> = BTreeMap::new();
    for record in &binaries {
        let dir = workspace_package_dir(&args.workspace_root, &record.package_root);
        let package_id =
            workspace_package_id(&dir, &record.package_name, &record.package_version);
        packages.entry(package_id).or_insert_with(|| WorkspacePackage {
            name: record.package_name.clone(),
            version: record.package_version.clone(),
            edition: record.edition.clone(),
            dir,
        });
    }
    for record in &non_test_binaries {
        if !packages.contains_key(&record.package_id) {
            color_eyre::eyre::bail!(
                "nextest-metadata-workspace: non-test binary {} names unknown package id {}; known: {}",
                record.name,
                record.package_id,
                packages.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    for package_id in build_script_out_dirs.keys() {
        if !packages.contains_key(package_id) {
            color_eyre::eyre::bail!(
                "nextest-metadata-workspace: build-script-out-dirs names unknown package id {package_id}; known: {}",
                packages.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }

    let target_directory = args.workspace_root.join("target").display().to_string();
    let cargo_metadata = workspace_cargo_metadata(args, &packages, &target_directory);
    let binaries_metadata = workspace_binaries_metadata(
        args,
        &binaries,
        &non_test_binaries,
        &build_script_out_dirs,
        &target_directory,
    )?;

    write_json(&args.cargo_metadata, &cargo_metadata)?;
    write_json(&args.binaries_metadata, &binaries_metadata)?;

    Ok(())
}

fn workspace_cargo_metadata(
    args: &NextestMetadataWorkspaceArgs,
    packages: &BTreeMap<String, WorkspacePackage>,
    target_directory: &str,
) -> serde_json::Value {
    let package_ids: Vec<&str> = packages.keys().map(String::as_str).collect();
    let cargo_packages: Vec<serde_json::Value> = packages
        .iter()
        .map(|(package_id, package)| {
            // One synthetic lib target per package: nextest only walks the
            // package graph for names, manifest dirs (test cwds after
            // `--workspace-remap`), and workspace membership.
            let cargo_target = serde_json::json!({
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": &package.name,
                "src_path": package.dir.join("src/lib.rs").display().to_string(),
                "edition": &package.edition,
                "doc": true,
                "doctest": false,
                "test": true
            });
            serde_json::json!({
                "name": &package.name,
                "version": &package.version,
                "id": package_id,
                "source": null,
                "dependencies": [],
                "features": {},
                "manifest_path": package.dir.join("Cargo.toml").display().to_string(),
                "edition": &package.edition,
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
            })
        })
        .collect();

    serde_json::json!({
        "version": 1,
        "workspace_root": args.workspace_root.display().to_string(),
        "target_directory": target_directory,
        "workspace_members": package_ids,
        "workspace_default_members": package_ids,
        "resolve": null,
        "metadata": null,
        "packages": cargo_packages
    })
}

fn workspace_binaries_metadata(
    args: &NextestMetadataWorkspaceArgs,
    binaries: &[WorkspaceTestBinary],
    non_test_binaries: &[WorkspaceNonTestBinary],
    build_script_out_dirs: &BTreeMap<String, String>,
    target_directory: &str,
) -> color_eyre::Result<serde_json::Value> {
    let mut rust_binaries = serde_json::Map::new();
    for record in binaries {
        let dir = workspace_package_dir(&args.workspace_root, &record.package_root);
        let package_id =
            workspace_package_id(&dir, &record.package_name, &record.package_version);
        let binary_id = nextest_binary_id(&record.package_name, &record.kind, &record.target_name);
        let binary = serde_json::json!({
            "binary-id": &binary_id,
            "binary-name": &record.target_name,
            "package-id": package_id,
            "kind": &record.kind,
            "binary-path": &record.binary_path,
            "build-platform": "target"
        });
        if rust_binaries.insert(binary_id.clone(), binary).is_some() {
            color_eyre::eyre::bail!(
                "nextest-metadata-workspace: two test binaries map to nextest binary id {binary_id} (second: target {} of package {})",
                record.target_name,
                record.package_name
            );
        }
    }

    let mut non_test_by_package: BTreeMap<&str, Vec<serde_json::Value>> = BTreeMap::new();
    for record in non_test_binaries {
        non_test_by_package
            .entry(record.package_id.as_str())
            .or_default()
            .push(serde_json::json!({
                "name": &record.name,
                "kind": &record.kind,
                "path": &record.path
            }));
    }

    let build_meta = nextest_build_meta(
        &args.target_triple,
        &args.rust_libdir.display().to_string(),
        target_directory,
        &serde_json::json!(non_test_by_package),
        build_script_out_dirs,
        &args.linked_paths,
    );

    Ok(serde_json::json!({
        "rust-build-meta": build_meta,
        "rust-binaries": rust_binaries
    }))
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
            embed_metadata: !args.no_embed_metadata,
            rmeta_stability_flags: args.rmeta_stability_flags,
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
        Command::NextestMetadataWorkspace(args) => nextest_metadata_workspace(&args),
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

    fn workspace_args(
        tmp: &std::path::Path,
        binaries_json: &str,
        non_test_binaries_json: Option<&str>,
    ) -> NextestMetadataWorkspaceArgs {
        let binaries = tmp.join("binaries.json");
        std::fs::write(&binaries, binaries_json).expect("write binaries input");
        let non_test_binaries = non_test_binaries_json.map(|json| {
            let path = tmp.join("non-test-binaries.json");
            std::fs::write(&path, json).expect("write non-test binaries input");
            path
        });
        NextestMetadataWorkspaceArgs {
            workspace_root: PathBuf::from("/workspace"),
            binaries,
            non_test_binaries,
            build_script_out_dirs: None,
            linked_paths: vec!["native=/nix/store/some-lib/lib".to_owned()],
            target_triple: "x86_64-unknown-linux-musl".to_owned(),
            rust_libdir: PathBuf::from(
                "/nix/store/rust/lib/rustlib/x86_64-unknown-linux-musl/lib",
            ),
            cargo_metadata: tmp.join("cargo-metadata.json"),
            binaries_metadata: tmp.join("binaries-metadata.json"),
        }
    }

    fn read_output(path: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read output JSON"))
            .expect("parse output JSON")
    }

    // Two packages, three binaries: alpha's lib unittest binary keys by bare
    // package name, its integration test by `package::target`, and beta's bin
    // unittest by `package::bin/target` — nextest's RustBinaryId spellings.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn nextest_metadata_workspace_writes_consistent_metadata() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let args = workspace_args(
            tmp.path(),
            r#"[
                {
                  "target-name": "alpha",
                  "package-name": "alpha",
                  "package-version": "0.1.0",
                  "package-root": "crates/alpha",
                  "kind": "lib",
                  "edition": "2024",
                  "binary-path": "/nix/store/alpha-test/bin/alpha"
                },
                {
                  "target-name": "alpha_it",
                  "package-name": "alpha",
                  "package-version": "0.1.0",
                  "package-root": "crates/alpha",
                  "kind": "test",
                  "edition": "2024",
                  "binary-path": "/nix/store/alpha-it/bin/alpha_it"
                },
                {
                  "target-name": "beta-cli",
                  "package-name": "beta-pkg",
                  "package-version": "0.2.0",
                  "package-root": "crates/beta",
                  "kind": "bin",
                  "edition": "2021",
                  "binary-path": "/nix/store/beta-test/bin/beta-cli"
                }
            ]"#,
            Some(
                r#"[
                {
                  "package-id": "path+file:///workspace/crates/beta#beta-pkg@0.2.0",
                  "name": "beta-cli",
                  "kind": "bin-exe",
                  "path": "/nix/store/beta/bin/beta-cli"
                }
            ]"#,
            ),
        );

        nextest_metadata_workspace(&args).expect("write workspace nextest metadata");

        let cargo = read_output(&args.cargo_metadata);
        let binaries = read_output(&args.binaries_metadata);

        let alpha_id = "path+file:///workspace/crates/alpha#alpha@0.1.0";
        let beta_id = "path+file:///workspace/crates/beta#beta-pkg@0.2.0";

        let rust_binaries = binaries["rust-binaries"]
            .as_object()
            .expect("rust-binaries is an object");
        assert_eq!(
            rust_binaries.keys().collect::<Vec<_>>(),
            ["alpha", "alpha::alpha_it", "beta-pkg::bin/beta-cli"]
        );
        assert_eq!(rust_binaries["alpha"]["kind"], "lib");
        assert_eq!(rust_binaries["alpha"]["binary-name"], "alpha");
        assert_eq!(rust_binaries["alpha::alpha_it"]["kind"], "test");
        assert_eq!(
            rust_binaries["alpha::alpha_it"]["binary-path"],
            "/nix/store/alpha-it/bin/alpha_it"
        );
        assert_eq!(
            rust_binaries["beta-pkg::bin/beta-cli"]["binary-name"],
            "beta-cli"
        );

        // The one invariant nextest hard-fails on: every package-id in
        // binaries-metadata byte-matches a package in cargo-metadata.
        let package_ids: Vec<&str> = cargo["packages"]
            .as_array()
            .expect("packages is an array")
            .iter()
            .map(|package| package["id"].as_str().expect("package id is a string"))
            .collect();
        assert_eq!(package_ids, [alpha_id, beta_id]);
        assert_eq!(
            cargo["workspace_members"],
            serde_json::json!([alpha_id, beta_id])
        );
        assert_eq!(
            cargo["workspace_default_members"],
            serde_json::json!([alpha_id, beta_id])
        );
        for binary in rust_binaries.values() {
            let package_id = binary["package-id"].as_str().expect("package-id string");
            assert!(package_ids.contains(&package_id));
        }

        assert_eq!(
            cargo["packages"][0]["manifest_path"],
            "/workspace/crates/alpha/Cargo.toml"
        );
        assert_eq!(cargo["packages"][1]["edition"], "2021");
        assert_eq!(cargo["workspace_root"], "/workspace");
        assert_eq!(cargo["target_directory"], "/workspace/target");

        let build_meta = &binaries["rust-build-meta"];
        assert_eq!(build_meta["target-directory"], "/workspace/target");
        assert_eq!(
            build_meta["non-test-binaries"][beta_id][0]["name"],
            "beta-cli"
        );
        assert_eq!(
            build_meta["non-test-binaries"][beta_id][0]["kind"],
            "bin-exe"
        );
        assert_eq!(
            build_meta["linked-paths"],
            serde_json::json!(["native=/nix/store/some-lib/lib"])
        );
        assert_eq!(
            build_meta["platforms"]["host"]["platform"]["triple"],
            "x86_64-unknown-linux-musl"
        );
    }

    #[test]
    fn nextest_metadata_workspace_refuses_colliding_binary_ids() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let args = workspace_args(
            tmp.path(),
            r#"[
                {
                  "target-name": "alpha",
                  "package-name": "alpha",
                  "package-version": "0.1.0",
                  "package-root": "crates/alpha",
                  "kind": "lib",
                  "edition": "2024",
                  "binary-path": "/nix/store/alpha-test/bin/alpha"
                },
                {
                  "target-name": "alpha",
                  "package-name": "alpha",
                  "package-version": "0.1.0",
                  "package-root": "crates/alpha",
                  "kind": "rlib",
                  "edition": "2024",
                  "binary-path": "/nix/store/alpha-test-2/bin/alpha"
                }
            ]"#,
            None,
        );

        let error = nextest_metadata_workspace(&args).expect_err("colliding ids must refuse");
        assert!(error.to_string().contains("binary id alpha"));
    }

    #[test]
    fn nextest_metadata_workspace_refuses_an_empty_binary_list() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let args = workspace_args(tmp.path(), "[]", None);

        let error = nextest_metadata_workspace(&args).expect_err("empty export must refuse");
        assert!(error.to_string().contains("lists no test binaries"));
    }

    #[test]
    fn nextest_metadata_workspace_refuses_unknown_non_test_package_ids() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let args = workspace_args(
            tmp.path(),
            r#"[
                {
                  "target-name": "alpha",
                  "package-name": "alpha",
                  "package-version": "0.1.0",
                  "package-root": "crates/alpha",
                  "kind": "lib",
                  "edition": "2024",
                  "binary-path": "/nix/store/alpha-test/bin/alpha"
                }
            ]"#,
            Some(
                r#"[
                {
                  "package-id": "path+file:///workspace/crates/gamma#gamma@1.0.0",
                  "name": "gamma-cli",
                  "kind": "bin-exe",
                  "path": "/nix/store/gamma/bin/gamma-cli"
                }
            ]"#,
            ),
        );

        let error = nextest_metadata_workspace(&args).expect_err("unknown package id must refuse");
        assert!(error.to_string().contains("unknown package id"));
    }
}
