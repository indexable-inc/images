use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use askama::Template as _;
use color_eyre::eyre::{Result, WrapErr as _, eyre};
use serde::Deserialize;
use sha2::Digest as _;

use crate::model::{Unit, UnitGraph, UnitMode};
use crate::shell;

pub struct RenderOptions {
    pub workspace_root: PathBuf,
    pub vendor_root: Option<PathBuf>,
    pub cargo_lock_sources: CargoLockSources,
    pub content_addressed: bool,
    pub toolchain_id: Option<String>,
    pub deny_unused_crate_dependencies: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CargoLockSources {
    packages: Vec<CargoLockPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoLock {
    #[serde(default)]
    package: Vec<CargoLockPackageEntry>,
}

#[derive(Debug, Deserialize)]
struct CargoLockPackageEntry {
    name: String,
    version: String,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CargoManifest {
    package: Option<CargoManifestPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoManifestPackage {
    links: Option<String>,
    #[serde(default)]
    authors: Option<toml::Value>,
    #[serde(default)]
    description: Option<toml::Value>,
    #[serde(default)]
    homepage: Option<toml::Value>,
    #[serde(default)]
    repository: Option<toml::Value>,
    #[serde(default)]
    license: Option<toml::Value>,
    #[serde(default, rename = "license-file")]
    license_file: Option<toml::Value>,
    #[serde(default, rename = "rust-version")]
    rust_version: Option<toml::Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CargoLockPackage {
    name: String,
    version: String,
    source: String,
}

impl CargoLockSources {
    pub fn from_path(path: &Path) -> Result<Self> {
        let lock = fs::read_to_string(path)
            .wrap_err_with(|| format!("reading Cargo.lock source map from {}", path.display()))?;
        Self::parse(&lock)
            .wrap_err_with(|| format!("parsing Cargo.lock source map from {}", path.display()))
    }

    fn parse(lock: &str) -> Result<Self> {
        let lock: CargoLock = toml::from_str(lock)?;
        let packages = lock
            .package
            .into_iter()
            .filter_map(|package| {
                let CargoLockPackageEntry {
                    name,
                    version,
                    source,
                } = package;
                source.map(|source| CargoLockPackage {
                    name,
                    version,
                    source,
                })
            })
            .collect();

        Ok(Self { packages })
    }

    fn source_for_unit(&self, unit: &Unit) -> Result<String> {
        let unit_name = unit.package_name();
        let unit_version = unit.package_version();
        let unit_source = external_source_from_pkg_id(&unit.pkg_id).ok_or_else(|| {
            eyre!(
                "external unit {} {} has package id without a registry, sparse, or git source: {}",
                unit_name,
                unit_version,
                unit.pkg_id
            )
        })?;

        let matches: Vec<_> = self
            .packages
            .iter()
            .filter(|package| {
                package.name == unit_name.as_ref()
                    && package.version == unit_version
                    && cargo_lock_source_matches_pkg_id(&unit_source, &package.source)
            })
            .collect();

        match matches.as_slice() {
            [package] => Ok(package.source.clone()),
            [] => Err(eyre!(
                "external unit {} {} has no matching Cargo.lock source for package id {}",
                unit_name,
                unit_version,
                unit.pkg_id
            )),
            packages => Err(eyre!(
                "external unit {} {} matches multiple Cargo.lock sources: {}",
                unit_name,
                unit_version,
                packages
                    .iter()
                    .map(|package| package.source.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

fn cargo_lock_source_matches_pkg_id(pkg_id_source: &str, cargo_lock_source: &str) -> bool {
    if pkg_id_source == cargo_lock_source {
        return true;
    }

    pkg_id_source.starts_with("git+")
        && cargo_lock_source
            .rsplit_once('#')
            .is_some_and(|(source_without_rev, _)| source_without_rev == pkg_id_source)
}

struct PreparedGraph {
    hashes: Vec<String>,
    names: Vec<String>,
    source_refs: Vec<String>,
    source_entries: BTreeMap<String, SourceEntry>,
    transitive_unit_deps: Vec<BTreeSet<usize>>,
    build_script_runs: BTreeMap<usize, BuildScriptRun>,
}

struct BuildScriptRun {
    compile_index: usize,
    dependency_runs: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceEntry {
    name: String,
    base: SourceBase,
    scope: SourceScope,
    root: PathBuf,
    relative: String,
    include_relatives: Vec<String>,
    source_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceBase {
    Workspace,
    WorkspaceClosure,
    VendorPackage,
    VendorClosure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceScope {
    Package,
    Closure,
}

struct ScopedSourceRoot {
    root: PathBuf,
    scope: SourceScope,
    relative: String,
    include_relatives: Vec<String>,
}

impl SourceBase {
    const fn label(self) -> &'static str {
        match self {
            Self::Workspace | Self::WorkspaceClosure => "workspace",
            Self::VendorPackage | Self::VendorClosure => "vendor",
        }
    }

    const fn audit_label(self) -> &'static str {
        match self {
            Self::Workspace | Self::WorkspaceClosure => "workspace",
            Self::VendorPackage => "vendor-package",
            Self::VendorClosure => "vendor-closure",
        }
    }
}

impl SourceScope {
    const fn audit_label(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Closure => "closure",
        }
    }
}

impl SourceEntry {
    fn nix_expr(&self) -> String {
        match self.base {
            SourceBase::Workspace => format!(
                "scopedWorkspaceSource {} {}",
                nix_attr(&self.name),
                nix_attr(&self.relative)
            ),
            SourceBase::WorkspaceClosure => format!(
                "scopedWorkspaceClosureSource {} {}",
                nix_attr(&self.name),
                nix_string_list(&self.include_relatives)
            ),
            SourceBase::VendorPackage => format!("vendorSources.{}", nix_attr(&self.source_key)),
            SourceBase::VendorClosure => format!(
                "scopedVendorClosureSource {} {}",
                nix_attr(&self.name),
                nix_string_list(&self.include_relatives)
            ),
        }
    }
}

#[derive(askama::Template)]
#[template(path = "units.nix.askama", escape = "none")]
struct UnitsNixTemplate {
    source_entries: String,
    source_audit_entries: String,
    unit_entries: String,
    clippy_unit_entries: String,
    policy_check_entries: String,
    roots: String,
    checked_roots: String,
    package_entries: String,
    binary_entries: String,
    library_entries: String,
    benchmark_entries: String,
    test_entries: String,
    doctest_entries: String,
    test_target_entries: String,
    doctest_target_entries: String,
    benchmark_target_entries: String,
    target_set_entries: String,
    default_entry: String,
}

pub fn render_units_nix(graph: &UnitGraph, options: &RenderOptions) -> Result<String> {
    graph.ensure_supported()?;
    let prepared = prepare_graph(graph, options)?;
    let template = UnitsNixTemplate {
        source_entries: render_source_entries(&prepared),
        source_audit_entries: render_source_audit_entries(&prepared),
        unit_entries: render_unit_entries(graph, options, &prepared)?,
        clippy_unit_entries: render_clippy_unit_entries(graph, options, &prepared)?,
        policy_check_entries: render_policy_check_entries(graph, options, &prepared)?,
        roots: render_roots(graph, &prepared),
        checked_roots: render_checked_roots(graph, &prepared),
        package_entries: render_root_entries(graph, &prepared, |_| true),
        binary_entries: render_root_entries(graph, &prepared, Unit::is_bin),
        library_entries: render_root_entries(graph, &prepared, Unit::is_library),
        benchmark_entries: render_benchmark_entries(graph, &prepared),
        test_entries: render_test_entries(graph, &prepared),
        doctest_entries: render_doctest_entries(graph, &prepared),
        test_target_entries: render_test_target_entries(graph, &prepared),
        doctest_target_entries: render_doctest_target_entries(graph, &prepared)?,
        benchmark_target_entries: render_benchmark_target_entries(graph, &prepared),
        target_set_entries: render_target_sets(graph, &prepared),
        default_entry: render_default_entry(graph, &prepared),
    };

    Ok(template.render()?)
}

fn render_unit_entries(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
) -> Result<String> {
    let mut entries = String::new();
    for (run_index, build_script_run) in &prepared.build_script_runs {
        write!(
            entries,
            "    {} = mkUnit {};\n\n",
            prepared.unit_attr(*run_index),
            render_build_script_run(graph, options, prepared, *run_index, build_script_run)?
        )?;
    }

    for (index, unit) in graph.units.iter().enumerate() {
        if unit.is_run_custom_build() {
            continue;
        }

        write!(
            entries,
            "    {} = mkUnit {};\n\n",
            prepared.unit_attr(index),
            render_rustc_unit(graph, options, prepared, index)?
        )?;
    }

    Ok(entries)
}

fn render_clippy_unit_entries(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
) -> Result<String> {
    let mut entries = String::new();
    for (index, unit) in graph.units.iter().enumerate() {
        if !is_clippy_unit_candidate(unit) {
            continue;
        }
        write!(
            entries,
            "    {} = mkClippyUnit {};\n\n",
            prepared.unit_attr(index),
            render_clippy_unit(graph, options, prepared, index)?
        )?;
    }
    Ok(entries)
}

// Clippy only lints workspace-owned code. Vendored crates (registry, sparse,
// git) compile under `--cap-lints warn` for a reason: we don't want a churning
// upstream to break the workspace lint gate. The run-custom-build unit
// executes `build.rs` (no source to lint), but the custom-build compile unit
// IS workspace Rust and the old `cargo clippy` workspace gate covered it, so
// it must keep getting clippy here.
fn is_clippy_unit_candidate(unit: &Unit) -> bool {
    !unit.is_run_custom_build() && !unit.is_external()
}

fn render_policy_check_entries(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
) -> Result<String> {
    let mut entries = String::new();
    if options.deny_unused_crate_dependencies {
        writeln!(
            entries,
            "    unusedCrateDependencies = {};",
            render_unused_crate_dependencies_check(graph, options, prepared)
        )?;
    }

    Ok(entries)
}

fn render_source_entries(prepared: &PreparedGraph) -> String {
    let mut entries = String::new();
    for (key, source) in &prepared.source_entries {
        let _ = writeln!(entries, "    {} = {};", nix_attr(key), source.nix_expr());
    }
    entries
}

fn render_source_audit_entries(prepared: &PreparedGraph) -> String {
    let mut entries = String::new();
    for (key, source) in &prepared.source_entries {
        let _ = writeln!(
            entries,
            "    {} = {{ base = {}; scope = {}; relative = {}; includeRelatives = {}; sourceKey = {}; }};",
            nix_attr(key),
            nix_attr(source.base.audit_label()),
            nix_attr(source.scope.audit_label()),
            nix_attr(&source.relative),
            nix_string_list(&source.include_relatives),
            nix_attr(&source.source_key),
        );
    }
    entries
}

fn render_default_entry(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    graph
        .roots
        .first()
        .map(|first_root| {
            format!(
                "  default = withPolicyChecks {};\n",
                prepared.unit_ref(*first_root)
            )
        })
        .unwrap_or_default()
}

impl PreparedGraph {
    fn unit_attr(&self, index: usize) -> String {
        nix_attr(&self.names[index])
    }

    fn unit_ref(&self, index: usize) -> String {
        format!("units.{}", self.unit_attr(index))
    }

    fn source_ref(&self, index: usize) -> String {
        format!("sources.{}", nix_attr(&self.source_refs[index]))
    }

    fn source_entry(&self, index: usize) -> Result<&SourceEntry> {
        let key = self
            .source_refs
            .get(index)
            .ok_or_else(|| eyre!("unit index {index} has no scoped source entry"))?;
        self.source_entries
            .get(key)
            .ok_or_else(|| eyre!("unit index {index} references missing scoped source {key}"))
    }
}

fn prepare_graph(graph: &UnitGraph, options: &RenderOptions) -> Result<PreparedGraph> {
    let mut hashes = vec![None; graph.units.len()];
    for index in 0..graph.units.len() {
        compute_hash(graph, options, index, &mut hashes)?;
    }
    let hashes: Vec<String> = hashes.into_iter().map(Option::unwrap).collect();

    let names: Vec<String> = graph
        .units
        .iter()
        .enumerate()
        .map(|(index, unit)| {
            if unit.is_run_custom_build() {
                format!(
                    "{}-build-script-run-{}-{}",
                    unit.package_name(),
                    unit.package_version(),
                    hashes[index]
                )
            } else {
                format!(
                    "{}-{}-{}",
                    unit.target.name,
                    unit.package_version(),
                    hashes[index]
                )
            }
        })
        .collect();

    let transitive_unit_deps = (0..graph.units.len())
        .map(|index| {
            let mut deps = BTreeSet::new();
            collect_transitive_unit_deps(graph, index, &mut deps)?;
            Ok(deps)
        })
        .collect::<Result<Vec<_>>>()?;

    let mut source_refs = Vec::with_capacity(graph.units.len());
    let mut source_entries = BTreeMap::new();
    for unit in &graph.units {
        let source = source_entry_for_unit(unit, options)?;
        let key = source.name.clone();
        source_refs.push(key.clone());
        source_entries.entry(key).or_insert(source);
    }

    let mut build_script_runs = BTreeMap::new();
    for (index, unit) in graph.units.iter().enumerate() {
        if !unit.is_run_custom_build() {
            continue;
        }

        let compile_index = unit
            .dependencies
            .iter()
            .map(|dep| dep.index)
            .find(|dep_index| {
                graph
                    .units
                    .get(*dep_index)
                    .is_some_and(Unit::is_custom_build_compile)
            })
            .ok_or_else(|| eyre!("build script run unit {index} has no compile dependency"))?;

        let dependency_runs = unit
            .dependencies
            .iter()
            .map(|dep| dep.index)
            .filter(|dep_index| {
                *dep_index != compile_index
                    && graph
                        .units
                        .get(*dep_index)
                        .is_some_and(Unit::is_run_custom_build)
            })
            .collect();

        build_script_runs.insert(
            index,
            BuildScriptRun {
                compile_index,
                dependency_runs,
            },
        );
    }

    Ok(PreparedGraph {
        hashes,
        names,
        source_refs,
        source_entries,
        transitive_unit_deps,
        build_script_runs,
    })
}

fn compute_hash(
    graph: &UnitGraph,
    options: &RenderOptions,
    index: usize,
    hashes: &mut [Option<String>],
) -> Result<String> {
    if let Some(hash) = &hashes[index] {
        return Ok(hash.clone());
    }

    let unit = graph.unit(index)?;
    let mut dependency_hashes = Vec::new();
    for dependency in &unit.dependencies {
        let dependency_unit = graph.unit(dependency.index)?;
        if dependency_unit.is_run_custom_build() {
            continue;
        }
        dependency_hashes.push(format!(
            "{}:{}:{}:{}",
            dependency.extern_crate_name,
            dependency.public,
            dependency.noprelude,
            compute_hash(graph, options, dependency.index, hashes)?
        ));
    }

    let hash = unit.identity_hash(&dependency_hashes, options.toolchain_id.as_deref());
    hashes[index] = Some(hash.clone());
    Ok(hash)
}

fn collect_transitive_unit_deps(
    graph: &UnitGraph,
    index: usize,
    deps: &mut BTreeSet<usize>,
) -> Result<()> {
    let unit = graph.unit(index)?;
    for dependency in &unit.dependencies {
        let dependency_unit = graph.unit(dependency.index)?;
        if dependency_unit.is_run_custom_build() {
            continue;
        }
        if deps.insert(dependency.index) {
            collect_transitive_unit_deps(graph, dependency.index, deps)?;
        }
    }

    Ok(())
}

// `Rustc` produces the build artifacts (rlib, bin, test binary).
// `ClippyDriver` runs the same compilation through `clippy-driver` so lints
// fire per unit, emitting metadata only — no link step, no binary output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Driver {
    Rustc,
    ClippyDriver,
}

impl Driver {
    const fn binary(self) -> &'static str {
        match self {
            Self::Rustc => "rustc",
            Self::ClippyDriver => "clippy-driver",
        }
    }
}

fn render_rustc_unit(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
    index: usize,
) -> Result<String> {
    let unit = &graph.units[index];
    let mut attrs = Attrs::new();

    attrs.string("pname", &unit.target.name);
    attrs.string("version", unit.package_version());
    attrs.expr("src", &prepared.source_ref(index));
    let native_build_inputs = if collects_unused_crate_dependencies(unit, options) {
        "[ rustToolchain pkgs.jq ] ++ extraNativeBuildInputs"
    } else {
        "[ rustToolchain ] ++ extraNativeBuildInputs"
    };
    attrs.expr("nativeBuildInputs", native_build_inputs);
    attrs.expr(
        "buildInputs",
        &render_build_inputs(graph, prepared, index, unit_build_script_run(graph, index)),
    );
    attrs.bool("dontStrip", true);
    if options.content_addressed {
        attrs.bool("__contentAddressed", true);
        attrs.string("outputHashMode", "recursive");
        attrs.string("outputHashAlgo", "sha256");
    }
    attrs.multiline(
        "buildPhase",
        &render_driver_build_phase(graph, options, prepared, index, Driver::Rustc)?,
    );
    attrs.multiline(
        "installPhase",
        &render_install_phase(unit, options, &prepared.hashes[index]),
    );

    Ok(attrs.render())
}

fn render_clippy_unit(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
    index: usize,
) -> Result<String> {
    let unit = &graph.units[index];
    let mut attrs = Attrs::new();

    attrs.string("pname", &format!("{}-clippy", unit.target.name));
    attrs.string("version", unit.package_version());
    attrs.expr("src", &prepared.source_ref(index));
    // `clippy-driver` rides the rustToolchain (which carries the matching
    // rustc); the clippy package only needs to be on PATH for the driver
    // binary. Callers append it via `extraClippyNativeBuildInputs`.
    attrs.expr(
        "nativeBuildInputs",
        "[ rustToolchain ] ++ extraNativeBuildInputs ++ extraClippyNativeBuildInputs",
    );
    // Clippy units link metadata-only against the build units' rlibs, just
    // like the build units do. They never depend on other clippy units, so a
    // touched source file only invalidates that unit's clippy plus everything
    // downstream that links its rlib.
    attrs.expr(
        "buildInputs",
        &render_build_inputs(graph, prepared, index, unit_build_script_run(graph, index)),
    );
    attrs.bool("dontStrip", true);
    if options.content_addressed {
        attrs.bool("__contentAddressed", true);
        attrs.string("outputHashMode", "recursive");
        attrs.string("outputHashAlgo", "sha256");
    }
    attrs.multiline(
        "buildPhase",
        &render_driver_build_phase(graph, options, prepared, index, Driver::ClippyDriver)?,
    );
    attrs.multiline("installPhase", "mkdir -p $out\n");

    Ok(attrs.render())
}

fn unit_build_script_run(graph: &UnitGraph, index: usize) -> Option<usize> {
    let unit = &graph.units[index];
    unit.dependencies
        .iter()
        .map(|dep| dep.index)
        .find(|dep_index| {
            graph.units.get(*dep_index).is_some_and(|dep_unit| {
                dep_unit.is_run_custom_build() && dep_unit.pkg_id == unit.pkg_id
            })
        })
}

fn render_build_inputs(
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    index: usize,
    build_script_run: Option<usize>,
) -> String {
    let mut refs: Vec<String> = graph.units[index]
        .dependencies
        .iter()
        .filter_map(|dep| {
            let dep_unit = &graph.units[dep.index];
            (!dep_unit.is_run_custom_build())
                .then(|| format!("units.{}", nix_attr(&prepared.names[dep.index])))
        })
        .collect();

    if let Some(run_index) = build_script_run {
        refs.push(format!("units.{}", nix_attr(&prepared.names[run_index])));
    }

    if refs.is_empty() {
        "[]".to_string()
    } else {
        format!("[ {} ]", refs.join(" "))
    }
}

// Cargo sets `CARGO_BIN_EXE_<name>` for integration tests and benchmarks.
// The unit graph exposes same-package build-mode bin targets as dependencies;
// test-harness bin units and integration-test binaries are runnable outputs too,
// but they are not the executable Cargo points this variable at.
fn same_package_bins(graph: &UnitGraph, index: usize) -> Vec<(String, usize)> {
    let unit = &graph.units[index];
    if !needs_bin_exe_env(unit) {
        return Vec::new();
    }
    unit.dependencies
        .iter()
        .filter_map(|dependency| {
            let candidate = graph.units.get(dependency.index)?;
            is_bin_exe_candidate(unit, candidate)
                .then(|| (candidate.target.name.clone(), dependency.index))
        })
        .collect()
}

fn needs_bin_exe_env(unit: &Unit) -> bool {
    matches!(unit.mode, UnitMode::Test)
        && unit
            .target
            .kind
            .iter()
            .any(|kind| matches!(kind.as_str(), "test" | "bench"))
}

fn is_bin_exe_candidate(unit: &Unit, candidate: &Unit) -> bool {
    candidate.pkg_id == unit.pkg_id
        && candidate.mode == UnitMode::Build
        && candidate.target.has_kind("bin")
        && candidate.platform == unit.platform
}

fn render_driver_build_phase(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
    index: usize,
    driver: Driver,
) -> Result<String> {
    let unit = &graph.units[index];
    let source = prepared.source_entry(index)?;
    let mut script = String::new();

    let collect_unused_deps =
        driver == Driver::Rustc && collects_unused_crate_dependencies(unit, options);

    script.push_str("mkdir -p build\n");
    script.push_str("build_script_flags=()\n");
    script.push_str("rustc_env=()\n");
    script.push_str("rustc_args=()\n\n");
    script.push_str(&cargo_package_exports(unit)?);
    writeln!(
        script,
        "export CARGO_MANIFEST_DIR={}",
        shell::double_quote(&source_path_expr(source, &crate_root_for_unit(unit))?)
    )?;
    append_bin_exe_env(&mut script, graph, prepared, index)?;

    if let Some(run_index) = unit_build_script_run(graph, index) {
        let run_ref = format!("${{units.{}}}", nix_attr(&prepared.names[run_index]));
        append_build_script_flag_reader(&mut script, &run_ref, unit);
    }

    push_rustc_args(&mut script, unit, &prepared.hashes[index]);
    append_target_linker_arg(&mut script, unit);
    append_extra_rustc_args(&mut script, unit);

    for dep_index in &prepared.transitive_unit_deps[index] {
        let dep = &graph.units[*dep_index];
        if dep.is_bin() {
            continue;
        }
        writeln!(
            script,
            "rustc_args+=( -L \"dependency=${{units.{}}}/lib\" )",
            nix_attr(&prepared.names[*dep_index])
        )?;
    }

    if unit.is_proc_macro() {
        script.push_str("rustc_args+=( --extern proc_macro )\n");
    }
    if collect_unused_deps {
        script.push_str("rustc_args+=( --error-format=json --json=unused-externs-silent )\n");
        push_arg(&mut script, "-W");
        push_arg(&mut script, "unused-crate-dependencies");
    }

    for dependency in &unit.dependencies {
        let dep_unit = &graph.units[dependency.index];
        if dep_unit.is_run_custom_build() || dep_unit.is_bin() {
            continue;
        }
        writeln!(
            script,
            "rustc_args+=( --extern \"{}=$(cat ${{units.{}}}/nix-support/extern-path)\" )",
            dependency.extern_crate_name,
            nix_attr(&prepared.names[dependency.index])
        )?;
    }

    let source_path = source_path_expr(source, Path::new(&unit.target.src_path))?;
    writeln!(
        script,
        "rustc_args+=( {} )",
        shell::double_quote(&source_path)
    )?;

    match driver {
        Driver::Rustc => {
            if unit.is_bin() || unit.is_test() {
                writeln!(
                    script,
                    "rustc_args+=( -o {} )",
                    shell::quote(&format!("build/{}", unit.target.name))
                )?;
            } else {
                script.push_str("rustc_args+=( --out-dir build )\n");
                if unit.is_proc_macro() {
                    script.push_str("rustc_args+=( --emit dep-info,link )\n");
                } else {
                    script.push_str("rustc_args+=( --emit dep-info,metadata,link )\n");
                }
            }
        }
        Driver::ClippyDriver => {
            // Clippy only needs MIR. Skip codegen and linking entirely.
            script.push_str("rustc_args+=( --out-dir build )\n");
            script.push_str("rustc_args+=( --emit dep-info,metadata )\n");
        }
    }

    script.push_str("rustc_args+=( \"''${build_script_flags[@]}\" )\n");
    if driver == Driver::ClippyDriver {
        // Inject the workspace's clippy lint policy (-D/-W/-A clippy::...)
        // at the rustc-args end so they override any earlier defaults.
        script.push_str("rustc_args+=( ${pkgs.lib.escapeShellArgs extraClippyLintArgs} )\n");
    }
    append_driver_invocation(&mut script, driver, collect_unused_deps);

    Ok(script)
}

fn append_bin_exe_env(
    script: &mut String,
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    index: usize,
) -> Result<()> {
    for (bin_name, bin_index) in same_package_bins(graph, index) {
        let env_name = format!("CARGO_BIN_EXE_{bin_name}");
        writeln!(
            script,
            "rustc_env+=( {} )",
            shell::quote(&format!(
                "{env_name}=${{units.{}}}/bin/{bin_name}",
                nix_attr(&prepared.names[bin_index])
            ))
        )?;
    }

    Ok(())
}

fn append_driver_invocation(script: &mut String, driver: Driver, collect_unused_deps: bool) {
    let binary = driver.binary();
    if collect_unused_deps {
        // `collect_unused_deps` is only ever true for the rustc driver; the
        // diagnostics-capture path is rustc-specific and reuses the rustc
        // unused-extern JSON shape.
        debug_assert_eq!(driver, Driver::Rustc);
        script.push_str("rustc_diagnostics=build/rustc-diagnostics.jsonl\n");
        script.push_str("set +e\n");
        script.push_str("set -x\n");
        let _ = writeln!(
            script,
            "env \"''${{rustc_env[@]}}\" {binary} \"''${{rustc_args[@]}}\" 2> \"$rustc_diagnostics\""
        );
        script.push_str("rustc_status=$?\n");
        script.push_str("set +x\n");
        script.push_str("set -e\n");
        script.push_str("cat \"$rustc_diagnostics\" >&2\n");
        script.push_str("if [ \"$rustc_status\" -ne 0 ]; then\n");
        script.push_str("  exit \"$rustc_status\"\n");
        script.push_str("fi\n");
        script.push_str(
            r#"jq -r 'select(."$message_type" == "unused_extern") | .unused_extern_names[]' "$rustc_diagnostics" | sort -u > build/unused-crate-dependencies
"#,
        );
    } else {
        script.push_str("set -x\n");
        let _ = writeln!(
            script,
            "env \"''${{rustc_env[@]}}\" {binary} \"''${{rustc_args[@]}}\""
        );
    }
}

fn collects_unused_crate_dependencies(unit: &Unit, options: &RenderOptions) -> bool {
    options.deny_unused_crate_dependencies && !unit.is_external()
}

fn push_rustc_args(script: &mut String, unit: &Unit, hash: &str) {
    push_arg(script, "--crate-name");
    push_arg(script, &unit.target.name.replace('-', "_"));
    push_arg(script, "--edition");
    push_arg(script, &unit.target.edition);

    for crate_type in &unit.target.crate_types {
        push_arg(script, "--crate-type");
        push_arg(script, crate_type);
    }
    if unit.is_proc_macro() {
        push_arg(script, "-C");
        push_arg(script, "prefer-dynamic");
    }

    push_codegen(script, "opt-level", &unit.profile.opt_level);
    push_codegen(script, "debuginfo", unit.profile.debuginfo.rustc_value());
    if let Some(lto) = lto_for_unit(unit) {
        push_codegen(script, "lto", lto);
    }
    if let Some(codegen_units) = unit.profile.codegen_units {
        push_codegen(script, "codegen-units", &codegen_units.to_string());
    }
    push_codegen(
        script,
        "debug-assertions",
        if unit.profile.debug_assertions {
            "yes"
        } else {
            "no"
        },
    );
    push_codegen(
        script,
        "overflow-checks",
        if unit.profile.overflow_checks {
            "yes"
        } else {
            "no"
        },
    );
    push_arg(script, "-C");
    push_arg(
        script,
        &format!("panic={}", unit.profile.panic.rustc_value()),
    );
    if let Some(strip) = unit.profile.strip.rustc_value() {
        push_arg(script, "-C");
        push_arg(script, &format!("strip={strip}"));
    }
    if let Some(split_debuginfo) = &unit.profile.split_debuginfo {
        push_arg(script, "-C");
        push_arg(script, &format!("split-debuginfo={split_debuginfo}"));
    }
    if unit.profile.rpath {
        push_arg(script, "-C");
        push_arg(script, "rpath=yes");
    }
    push_codegen(script, "metadata", hash);
    push_codegen(script, "extra-filename", &format!("-{hash}"));

    for rustflag in &unit.profile.rustflags {
        push_arg(script, rustflag);
    }
    for rustflag in &unit.lint_rustflags {
        push_arg(script, rustflag);
    }
    for arg in &unit.check_cfg_args {
        push_arg(script, arg);
    }
    for feature in &unit.features {
        push_arg(script, "--cfg");
        push_arg(script, &format!("feature=\"{feature}\""));
    }
    if unit.uses_test_harness() {
        push_arg(script, "--test");
    } else if unit.mode == UnitMode::Test {
        push_arg(script, "--cfg");
        push_arg(script, "test");
    }
    if let Some(platform) = &unit.platform {
        push_arg(script, "--target");
        push_arg(script, platform);
    }
    if unit.is_external() {
        push_arg(script, "--cap-lints");
        push_arg(script, "warn");
    }
}

fn lto_for_unit(unit: &Unit) -> Option<&'static str> {
    let allowed = unit
        .target
        .crate_types
        .iter()
        .all(|crate_type| matches!(crate_type.as_str(), "bin" | "cdylib" | "staticlib"));
    allowed.then(|| unit.profile.lto.rustc_value()).flatten()
}

fn push_codegen(script: &mut String, key: &str, value: &str) {
    push_arg(script, "-C");
    push_arg(script, &format!("{key}={value}"));
}

fn push_arg(script: &mut String, value: &str) {
    let _ = writeln!(script, "rustc_args+=( {} )", shell::quote(value));
}

fn append_target_linker_arg(script: &mut String, unit: &Unit) {
    let Some(platform) = &unit.platform else {
        return;
    };
    let env_name = cargo_target_linker_env_name(platform);
    let _ = writeln!(script, "if [ \"''${{{env_name}+x}}\" = x ]; then");
    let _ = writeln!(script, "  rustc_args+=( -C \"linker=''${{{env_name}}}\" )");
    script.push_str("fi\n");
}

fn cargo_target_linker_env_name(target: &str) -> String {
    let mut env_name = String::from("CARGO_TARGET_");
    for byte in target.bytes() {
        match byte {
            b'a'..=b'z' => env_name.push(char::from(byte.to_ascii_uppercase())),
            b'A'..=b'Z' | b'0'..=b'9' | b'_' => env_name.push(char::from(byte)),
            _ => env_name.push('_'),
        }
    }
    env_name.push_str("_LINKER");
    env_name
}

fn append_extra_rustc_args(script: &mut String, unit: &Unit) {
    let platform = unit
        .platform
        .as_ref()
        .map_or_else(|| "null".to_string(), |platform| nix_attr(platform));
    let _ = writeln!(script, "${{renderExtraRustcArgs {platform}}}");
}

fn append_build_script_flag_reader(script: &mut String, run_ref: &str, unit: &Unit) {
    let quoted_run_ref = format!("\"{run_ref}\"");
    let snippets = [
        ("rustc-cfg", "--cfg"),
        ("rustc-link-lib", "-l"),
        ("rustc-link-search", "-L"),
    ];

    script.push('\n');
    for (file, flag) in snippets {
        let flag_arg = shell::quote(flag);
        let _ = writeln!(
            script,
            "if [ -f {quoted_run_ref}/{file} ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && build_script_flags+=( {flag_arg} \"$line\" )\n  done < {quoted_run_ref}/{file}\nfi",
        );
    }
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/rustc-cdylib-link-arg ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && build_script_flags+=( -C \"link-arg=$line\" )\n  done < {quoted_run_ref}/rustc-cdylib-link-arg\nfi",
    );
    append_link_arg_reader(script, &quoted_run_ref, "rustc-link-arg");
    if unit.is_benchmark() {
        append_link_arg_reader(script, &quoted_run_ref, "rustc-link-arg-benches");
    } else if unit.is_test() {
        append_link_arg_reader(script, &quoted_run_ref, "rustc-link-arg-tests");
    } else if unit.is_bin() {
        append_link_arg_reader(script, &quoted_run_ref, "rustc-link-arg-bins");
    }
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/rustc-env ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && export \"$line\"\n  done < {quoted_run_ref}/rustc-env\nfi",
    );
    let _ = writeln!(script, "export OUT_DIR={quoted_run_ref}/out-dir\n");
}

fn append_link_arg_reader(script: &mut String, quoted_run_ref: &str, file: &str) {
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/{file} ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && build_script_flags+=( -C \"link-arg=$line\" )\n  done < {quoted_run_ref}/{file}\nfi",
    );
}

fn render_install_phase(unit: &Unit, options: &RenderOptions, hash: &str) -> String {
    let unused_crate_dependencies_install = if collects_unused_crate_dependencies(unit, options) {
        "\
if [ -s build/unused-crate-dependencies ]; then
  cp build/unused-crate-dependencies $out/nix-support/unused-crate-dependencies
fi
"
    } else {
        ""
    };

    if unit.is_bin() || unit.is_test() {
        format!(
            "\
mkdir -p $out/bin $out/nix-support
cp {} $out/bin/{}
chmod 755 $out/bin/{}
{unused_crate_dependencies_install}
",
            shell::quote(&format!("build/{}", unit.target.name)),
            shell::quote(&unit.target.name),
            shell::quote(&unit.target.name)
        )
    } else {
        let lib_name = unit.target.name.replace('-', "_");
        format!(
            "\
mkdir -p $out/lib $out/nix-support
cp -R build/* $out/lib/
extern_path=\"\"
for artifact in \\
  \"$out/lib/lib{lib_name}-{hash}.rlib\" \\
  \"$out/lib/lib{lib_name}-{hash}.so\" \\
  \"$out/lib/lib{lib_name}-{hash}.dylib\" \\
  \"$out/lib/{lib_name}-{hash}.dll\" \\
  \"$out/lib/lib{lib_name}-{hash}.rmeta\"; do
  if [ -f \"$artifact\" ]; then
    extern_path=\"$artifact\"
    break
  fi
done
[ -n \"$extern_path\" ] && printf '%s\\n' \"$extern_path\" > $out/nix-support/extern-path
{unused_crate_dependencies_install}
"
        )
    }
}

fn render_build_script_run(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
    run_index: usize,
    build_script_run: &BuildScriptRun,
) -> Result<String> {
    let run_unit = &graph.units[run_index];
    let compile_unit = &graph.units[build_script_run.compile_index];
    let mut attrs = Attrs::new();

    attrs.string(
        "pname",
        &format!("{}-build-script-output", run_unit.package_name()),
    );
    attrs.string("version", run_unit.package_version());
    attrs.expr("src", &prepared.source_ref(run_index));
    attrs.expr(
        "nativeBuildInputs",
        "[ rustToolchain ] ++ extraNativeBuildInputs",
    );

    let mut inputs = vec![format!(
        "units.{}",
        nix_attr(&prepared.names[build_script_run.compile_index])
    )];
    inputs.extend(
        build_script_run
            .dependency_runs
            .iter()
            .map(|index| format!("units.{}", nix_attr(&prepared.names[*index]))),
    );
    attrs.expr("buildInputs", &format!("[ {} ]", inputs.join(" ")));
    attrs.bool("dontStrip", true);
    if options.content_addressed {
        attrs.bool("__contentAddressed", true);
        attrs.string("outputHashMode", "recursive");
        attrs.string("outputHashAlgo", "sha256");
    }
    attrs.multiline(
        "buildPhase",
        &render_build_script_run_phase(
            graph,
            prepared,
            run_index,
            run_unit,
            compile_unit,
            build_script_run.compile_index,
            build_script_run,
        )?,
    );
    attrs.multiline("installPhase", "true\n");

    Ok(attrs.render())
}

#[allow(clippy::too_many_lines)]
fn render_build_script_run_phase(
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    run_index: usize,
    run_unit: &Unit,
    compile_unit: &Unit,
    compile_index: usize,
    build_script_run: &BuildScriptRun,
) -> Result<String> {
    let mut script = String::new();
    let source = prepared.source_entry(run_index)?;
    let compile_ref = format!("${{units.{}}}", nix_attr(&prepared.names[compile_index]));

    script.push_str("mkdir -p $out/out-dir\n");
    script.push_str("export OUT_DIR=$out/out-dir\n");
    ensure_source_contains_unit(source, run_unit)?;
    writeln!(
        script,
        "export CARGO_MANIFEST_DIR={}",
        shell::double_quote(&source_path_expr(source, &crate_root_for_unit(run_unit))?)
    )?;
    script.push_str("export RUSTC=\"$(type -p rustc)\"\n");
    script.push_str("HOST_TRIPLE=\"$($RUSTC -vV | sed -n 's/^host: //p')\"\n");
    script.push_str("export HOST=\"$HOST_TRIPLE\"\n");
    if let Some(platform) = &run_unit.platform {
        writeln!(script, "export TARGET={}", shell::quote(platform))?;
    } else {
        script.push_str("export TARGET=\"$HOST_TRIPLE\"\n");
    }
    writeln!(
        script,
        "export PROFILE={}",
        shell::quote(&run_unit.profile.name)
    )?;
    writeln!(
        script,
        "export OPT_LEVEL={}",
        shell::quote(&run_unit.profile.opt_level)
    )?;
    writeln!(
        script,
        "export DEBUG={}",
        shell::quote(if run_unit.profile.debuginfo.is_enabled() {
            "true"
        } else {
            "false"
        })
    )?;
    script.push_str(concat!("export NUM_JOBS=''", "${NIX_BUILD_CORES:-1}\n"));
    script.push_str(&cargo_package_exports(run_unit)?);
    script.push_str(&cargo_manifest_links_export(run_unit)?);
    append_cargo_feature_exports(&mut script, run_unit);
    append_cargo_cfg_exports(&mut script);
    append_dependency_metadata_exports(&mut script, graph, prepared, build_script_run)?;
    script.push_str("cd \"$CARGO_MANIFEST_DIR\"\n");
    script.push_str("build_script_stdout=$(mktemp)\n");
    script.push_str("build_script_stderr=$(mktemp)\n");
    script.push_str("set +e\n");
    writeln!(
        script,
        "{}/bin/{} > \"$build_script_stdout\" 2> \"$build_script_stderr\"",
        compile_ref,
        shell::quote(&compile_unit.target.name)
    )?;
    script.push_str("build_script_status=$?\n");
    script.push_str("set -e\n");
    script.push_str("cat \"$build_script_stderr\" >&2\n");
    script.push_str("if [ \"$build_script_status\" -ne 0 ]; then\n");
    script.push_str("  cat \"$build_script_stdout\" >&2\n");
    script.push_str("  exit \"$build_script_status\"\n");
    script.push_str("fi\n");
    script.push_str(
        r#"
while IFS= read -r line; do
  case "$line" in
    cargo::*)
      normalized="cargo:''${line#cargo::}"
      ;;
    *)
      normalized="$line"
      ;;
  esac

  case "$normalized" in
    cargo:rustc-cfg=*)
      printf '%s\n' "''${normalized#cargo:rustc-cfg=}" >> $out/rustc-cfg
      ;;
    cargo:rustc-link-lib=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-lib=}" >> $out/rustc-link-lib
      ;;
    cargo:rustc-link-search=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-search=}" >> $out/rustc-link-search
      ;;
    cargo:rustc-env=*)
      printf '%s\n' "''${normalized#cargo:rustc-env=}" >> $out/rustc-env
      ;;
    cargo:rustc-cdylib-link-arg=*)
      printf '%s\n' "''${normalized#cargo:rustc-cdylib-link-arg=}" >> $out/rustc-cdylib-link-arg
      ;;
    cargo:rustc-link-arg=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-arg=}" >> $out/rustc-link-arg
      ;;
    cargo:rustc-link-arg-benches=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-arg-benches=}" >> $out/rustc-link-arg-benches
      ;;
    cargo:rustc-link-arg-bins=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-arg-bins=}" >> $out/rustc-link-arg-bins
      ;;
    cargo:rustc-link-arg-tests=*)
      printf '%s\n' "''${normalized#cargo:rustc-link-arg-tests=}" >> $out/rustc-link-arg-tests
      ;;
    cargo:warning=*)
      printf '%s\n' "build script warning: ''${normalized#cargo:warning=}" >&2
      ;;
    cargo:rerun-if-changed=*|cargo:rerun-if-env-changed=*)
      ;;
    cargo:*)
      printf '%s\n' "''${normalized#cargo:}" >> $out/cargo-metadata
      ;;
  esac
done < "$build_script_stdout"
"#,
    );

    Ok(script)
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyPolicyKey {
    pkg_id: String,
    package_name: String,
    package_version: String,
    extern_crate_name: String,
}

fn render_unused_crate_dependencies_check(
    graph: &UnitGraph,
    options: &RenderOptions,
    prepared: &PreparedGraph,
) -> String {
    let mut dependency_units: BTreeMap<DependencyPolicyKey, BTreeSet<usize>> = BTreeMap::new();

    for (index, unit) in graph.units.iter().enumerate() {
        if unit.is_run_custom_build() || !collects_unused_crate_dependencies(unit, options) {
            continue;
        }

        for dependency in &unit.dependencies {
            let dep_unit = &graph.units[dependency.index];
            if dep_unit.is_run_custom_build() || dep_unit.is_bin() {
                continue;
            }

            dependency_units
                .entry(DependencyPolicyKey {
                    pkg_id: unit.pkg_id.clone(),
                    package_name: unit.package_name().to_string(),
                    package_version: unit.package_version().to_string(),
                    extern_crate_name: dependency.extern_crate_name.clone(),
                })
                .or_default()
                .insert(index);
        }
    }

    let mut script = String::new();
    script.push_str(
        "pkgs.runCommand \"cargo-unit-unused-crate-dependencies\" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''\n",
    );
    script.push_str("      failures=0\n");
    script.push_str("      check_unused() {\n");
    script.push_str("        package=\"$1\"\n");
    script.push_str("        dependency=\"$2\"\n");
    script.push_str("        shift 2\n");
    script.push_str("        unit_count=\"$#\"\n");
    script.push_str("        unused_count=0\n\n");
    script.push_str("        for unit in \"$@\"; do\n");
    script.push_str("          report=\"$unit/nix-support/unused-crate-dependencies\"\n");
    script.push_str(
        "          if [ -f \"$report\" ] && grep -Fxq \"$dependency\" \"$report\"; then\n",
    );
    script.push_str("            unused_count=$((unused_count + 1))\n");
    script.push_str("          fi\n");
    script.push_str("        done\n\n");
    script.push_str("        if [ \"$unused_count\" -eq \"$unit_count\" ]; then\n");
    script.push_str(
        "          printf 'unused dependency in %s: %s\\n' \"$package\" \"$dependency\" >&2\n",
    );
    script.push_str("          failures=1\n");
    script.push_str("        fi\n");
    script.push_str("      }\n\n");

    for (dependency, unit_indexes) in dependency_units {
        let unit_refs = unit_indexes
            .iter()
            .map(|index| format!("\"${{units.{}}}\"", nix_attr(&prepared.names[*index])))
            .collect::<Vec<_>>()
            .join(" ");
        let package = format!("{} {}", dependency.package_name, dependency.package_version);
        let _ = writeln!(
            script,
            "      check_unused {} {} {unit_refs}",
            shell::quote(&package),
            shell::quote(&dependency.extern_crate_name),
        );
    }

    script.push_str("\n      if [ \"$failures\" -ne 0 ]; then\n");
    script.push_str("        exit 1\n");
    script.push_str("      fi\n");
    script.push_str("      mkdir -p \"$out\"\n");
    script.push_str("    ''");
    script
}

fn cargo_package_exports(unit: &Unit) -> Result<String> {
    let mut script = String::new();
    let package_name = unit.package_name();
    let version = unit.package_version();
    let manifest = optional_cargo_manifest_package(unit)?;
    // Build scripts observe Cargo's split version fields, including the empty
    // prerelease string. ring uses CARGO_PKG_VERSION_PRE in its links invariant.
    let version_without_build_metadata = version.split_once('+').map_or(version, |(base, _)| base);
    let (version_core, version_pre) = version_without_build_metadata
        .split_once('-')
        .unwrap_or((version_without_build_metadata, ""));
    let mut version_parts = version_core.split('.');
    let major = version_parts.next().unwrap_or("0");
    let minor = version_parts.next().unwrap_or("0");
    let patch = version_parts.next().unwrap_or("0");

    let metadata = manifest
        .as_ref()
        .map(cargo_manifest_package_metadata)
        .unwrap_or_default();

    // CARGO_CRATE_NAME is the rust identifier rustc receives via `--crate-name`.
    // Cargo normalizes the target's name (`-` → `_`); crates like rmcp read this
    // via `env!()` at compile time.
    let crate_name = unit.target.name.replace('-', "_");
    let is_bin = unit.target.kind.iter().any(|kind| kind == "bin");

    for (name, value) in [
        ("CARGO_CRATE_NAME", crate_name.as_str()),
        ("CARGO_PKG_NAME", package_name.as_ref()),
        ("CARGO_PKG_VERSION", version),
        ("CARGO_PKG_VERSION_MAJOR", major),
        ("CARGO_PKG_VERSION_MINOR", minor),
        ("CARGO_PKG_VERSION_PATCH", patch),
        ("CARGO_PKG_VERSION_PRE", version_pre),
        ("CARGO_PKG_AUTHORS", metadata.authors.as_str()),
        ("CARGO_PKG_DESCRIPTION", metadata.description.as_str()),
        ("CARGO_PKG_HOMEPAGE", metadata.homepage.as_str()),
        ("CARGO_PKG_REPOSITORY", metadata.repository.as_str()),
        ("CARGO_PKG_LICENSE", metadata.license.as_str()),
        ("CARGO_PKG_LICENSE_FILE", metadata.license_file.as_str()),
        ("CARGO_PKG_RUST_VERSION", metadata.rust_version.as_str()),
    ] {
        let _ = writeln!(script, "export {name}={}", shell_env_value(value));
    }

    if is_bin {
        let _ = writeln!(
            script,
            "export CARGO_BIN_NAME={}",
            shell_env_value(&unit.target.name)
        );
    }

    Ok(script)
}

fn cargo_manifest_links_export(unit: &Unit) -> Result<String> {
    // Cargo injects package.links for build.rs. nix-cargo-unit runs build scripts
    // outside Cargo, and crates like ring panic when CARGO_MANIFEST_LINKS is absent.
    Ok(cargo_manifest_package(unit)?
        .and_then(|package| package.links)
        .map(|links| format!("export CARGO_MANIFEST_LINKS={}\n", shell_env_value(&links)))
        .unwrap_or_default())
}

fn shell_env_value(value: &str) -> String {
    nix_indented_string_fragment(&shell_double_quote_literal(value))
}

fn shell_double_quote_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn nix_indented_string_fragment(value: &str) -> String {
    value.replace("''", "'''").replace("${", "''${")
}

fn nix_indented_string(value: &str) -> String {
    format!("''\n{value}''")
}

#[derive(Default)]
struct CargoManifestPackageMetadata {
    authors: String,
    description: String,
    homepage: String,
    repository: String,
    license: String,
    license_file: String,
    rust_version: String,
}

fn cargo_manifest_package_metadata(package: &CargoManifestPackage) -> CargoManifestPackageMetadata {
    CargoManifestPackageMetadata {
        authors: manifest_string_list(package.authors.as_ref()).join(":"),
        description: manifest_string(package.description.as_ref()),
        homepage: manifest_string(package.homepage.as_ref()),
        repository: manifest_string(package.repository.as_ref()),
        license: manifest_string(package.license.as_ref()),
        license_file: manifest_string(package.license_file.as_ref()),
        rust_version: manifest_string(package.rust_version.as_ref()),
    }
}

fn manifest_string(value: Option<&toml::Value>) -> String {
    value
        .and_then(toml::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn manifest_string_list(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(toml::Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn optional_cargo_manifest_package(unit: &Unit) -> Result<Option<CargoManifestPackage>> {
    let manifest_path = crate_root_for_unit(unit).join("Cargo.toml");
    let manifest = match fs::read_to_string(&manifest_path) {
        Ok(manifest) => manifest,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .wrap_err_with(|| format!("reading package manifest {}", manifest_path.display()));
        }
    };

    cargo_manifest_package_from_str(&manifest)
        .wrap_err_with(|| format!("parsing package manifest {}", manifest_path.display()))
}

fn cargo_manifest_package(unit: &Unit) -> Result<Option<CargoManifestPackage>> {
    let manifest_path = crate_root_for_unit(unit).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .wrap_err_with(|| format!("reading package manifest {}", manifest_path.display()))?;

    cargo_manifest_package_from_str(&manifest)
        .wrap_err_with(|| format!("parsing package manifest {}", manifest_path.display()))
}

fn cargo_manifest_package_from_str(manifest: &str) -> Result<Option<CargoManifestPackage>> {
    let manifest: CargoManifest = toml::from_str(manifest)?;
    Ok(manifest.package)
}

#[cfg(test)]
fn cargo_manifest_package_links(manifest: &str) -> Result<Option<String>> {
    Ok(cargo_manifest_package_from_str(manifest)?.and_then(|package| package.links))
}

fn append_cargo_feature_exports(script: &mut String, unit: &Unit) {
    for feature in &unit.features {
        let _ = writeln!(script, "export {}=1", cargo_feature_env_name(feature));
    }
}

fn cargo_feature_env_name(feature: &str) -> String {
    let mut env_name = String::from("CARGO_FEATURE_");
    for byte in feature.bytes() {
        match byte {
            b'a'..=b'z' => env_name.push(char::from(byte.to_ascii_uppercase())),
            b'A'..=b'Z' | b'0'..=b'9' | b'_' => env_name.push(char::from(byte)),
            _ => env_name.push('_'),
        }
    }
    env_name
}

fn append_cargo_cfg_exports(script: &mut String) {
    // Cargo normally exports CARGO_CFG_* before build.rs. Direct build-script
    // execution has to synthesize them or target-sensitive crates like libm fail.
    script.push_str(
        r#"cargo_cfg_output=$(mktemp)
"$RUSTC" --print cfg --target "$TARGET" > "$cargo_cfg_output"
while IFS= read -r cargo_cfg_line; do
  case "$cargo_cfg_line" in
    *=*)
      cargo_cfg_key="''${cargo_cfg_line%%=*}"
      cargo_cfg_value="''${cargo_cfg_line#*=}"
      cargo_cfg_value="''${cargo_cfg_value%\"}"
      cargo_cfg_value="''${cargo_cfg_value#\"}"
      ;;
    *)
      cargo_cfg_key="$cargo_cfg_line"
      cargo_cfg_value=""
      ;;
  esac

  cargo_cfg_env="CARGO_CFG_$(printf '%s' "$cargo_cfg_key" | tr '[:lower:]-' '[:upper:]_')"
  if [ "''${!cargo_cfg_env+x}" = x ] && [ -n "$cargo_cfg_value" ]; then
    export "$cargo_cfg_env=''${!cargo_cfg_env},$cargo_cfg_value"
  else
    export "$cargo_cfg_env=$cargo_cfg_value"
  fi
done < "$cargo_cfg_output"
"#,
    );
}

fn append_dependency_metadata_exports(
    script: &mut String,
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    build_script_run: &BuildScriptRun,
) -> Result<()> {
    for dep_run_index in &build_script_run.dependency_runs {
        let dep_run_unit = &graph.units[*dep_run_index];
        let Some(links) = cargo_manifest_package(dep_run_unit)?.and_then(|package| package.links)
        else {
            continue;
        };
        let dep_run_ref = format!("${{units.{}}}", nix_attr(&prepared.names[*dep_run_index]));
        let env_prefix = cargo_links_env_prefix(&links);
        let _ = writeln!(
            script,
            r#"# Cargo exposes metadata from build-script dependencies through DEP_<links>_*.
# aws-lc-rs uses these variables to find the aws-lc-sys headers and link outputs.
if [ -f "{dep_run_ref}/cargo-metadata" ]; then
  while IFS= read -r cargo_metadata_line; do
    case "$cargo_metadata_line" in
      *=*)
        cargo_metadata_key="''${{cargo_metadata_line%%=*}}"
        cargo_metadata_value="''${{cargo_metadata_line#*=}}"
        cargo_metadata_env="DEP_{env_prefix}_$(printf '%s' "$cargo_metadata_key" | tr '[:lower:]-' '[:upper:]_')"
        export "$cargo_metadata_env=$cargo_metadata_value"
        ;;
    esac
  done < "{dep_run_ref}/cargo-metadata"
fi"#
        );
    }
    Ok(())
}

fn cargo_links_env_prefix(links: &str) -> String {
    links
        .chars()
        .map(|ch| match ch {
            'a'..='z' => ch.to_ascii_uppercase(),
            '-' => '_',
            _ => ch,
        })
        .collect()
}

fn crate_root_for_unit(unit: &Unit) -> PathBuf {
    let source = Path::new(&unit.target.src_path);
    if let Some(manifest_root) = nearest_manifest_root(source) {
        return manifest_root;
    }

    if source.file_name().is_some_and(|name| name == "build.rs") {
        return source.parent().unwrap_or(source).to_path_buf();
    }

    let raw = unit.target.src_path.as_str();
    if let Some((root, _)) = raw.split_once("/src/") {
        return PathBuf::from(root);
    }

    source.parent().unwrap_or(source).to_path_buf()
}

fn nearest_manifest_root(source: &Path) -> Option<PathBuf> {
    let mut dir = source.parent()?;
    loop {
        // Cargo sets CARGO_MANIFEST_DIR to the package root even when the
        // build script entrypoint is nested, as aws-lc-sys does with builder/main.rs.
        if dir.join("Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn source_entry_for_unit(unit: &Unit, options: &RenderOptions) -> Result<SourceEntry> {
    if unit.is_external() {
        let vendor_root = options.vendor_root.as_ref().ok_or_else(|| {
            eyre!(
                "external unit {} {} needs --vendor-root to scope its vendored source",
                unit.package_name(),
                unit.package_version()
            )
        })?;
        let scoped = vendored_source_root_for_unit(unit, vendor_root)?;

        let base = match scoped.scope {
            SourceScope::Package => SourceBase::VendorPackage,
            SourceScope::Closure => SourceBase::VendorClosure,
        };
        let source_key = vendor_source_key(unit, &options.cargo_lock_sources)?;

        return Ok(SourceEntry {
            name: source_name(base, unit, &source_key, &scoped.relative),
            base,
            scope: scoped.scope,
            root: scoped.root,
            relative: scoped.relative,
            include_relatives: scoped.include_relatives,
            source_key,
        });
    }

    let scoped = local_source_root_for_unit(unit, &options.workspace_root)?;

    let source_key = local_source_key(unit);

    Ok(SourceEntry {
        name: source_name(SourceBase::Workspace, unit, &source_key, &scoped.relative),
        base: match scoped.scope {
            SourceScope::Package => SourceBase::Workspace,
            SourceScope::Closure => SourceBase::WorkspaceClosure,
        },
        scope: scoped.scope,
        root: scoped.root,
        relative: scoped.relative,
        include_relatives: scoped.include_relatives,
        source_key,
    })
}

fn local_source_root_for_unit(unit: &Unit, workspace_root: &Path) -> Result<ScopedSourceRoot> {
    let package_root =
        local_package_root_from_pkg_id(&unit.pkg_id).unwrap_or_else(|| crate_root_for_unit(unit));
    relative_path_string(&package_root, workspace_root).map_err(|_| {
        eyre!(
            "local unit {} {} source root {} is outside workspace root {}",
            unit.package_name(),
            unit.package_version(),
            package_root.display(),
            workspace_root.display()
        )
    })?;

    let package_relative = relative_path_string(&package_root, workspace_root)?;
    let include_relatives = source_closure_relatives(&package_root, workspace_root)?;

    if include_relatives.len() > 1 || include_relatives.first() != Some(&package_relative) {
        return Ok(ScopedSourceRoot {
            root: workspace_root.to_path_buf(),
            scope: SourceScope::Closure,
            relative: package_relative,
            include_relatives,
        });
    }

    Ok(ScopedSourceRoot {
        root: package_root,
        scope: SourceScope::Package,
        relative: package_relative.clone(),
        include_relatives: vec![package_relative],
    })
}

fn vendored_source_root_for_unit(unit: &Unit, vendor_root: &Path) -> Result<ScopedSourceRoot> {
    let source = Path::new(&unit.target.src_path);
    let relative = source.strip_prefix(vendor_root).map_err(|_| {
        eyre!(
            "external unit {} {} source path {} is outside vendor root {}",
            unit.package_name(),
            unit.package_version(),
            source.display(),
            vendor_root.display()
        )
    })?;

    let crate_root = match relative.components().next() {
        Some(Component::Normal(component)) => vendor_root.join(component),
        _ => Err(eyre!(
            "external unit {} {} source path {} does not contain a vendored crate directory under {}",
            unit.package_name(),
            unit.package_version(),
            source.display(),
            vendor_root.display()
        ))?,
    };

    let crate_relative = relative_path_string(&crate_root, vendor_root)?;
    let include_relatives = source_closure_relatives(&crate_root, vendor_root)?;

    if include_relatives.len() > 1 || include_relatives.first() != Some(&crate_relative) {
        return Ok(ScopedSourceRoot {
            root: vendor_root.to_path_buf(),
            scope: SourceScope::Closure,
            relative: crate_relative,
            include_relatives,
        });
    }

    Ok(ScopedSourceRoot {
        root: crate_root,
        scope: SourceScope::Package,
        relative: crate_relative.clone(),
        include_relatives: vec![crate_relative],
    })
}

fn relative_path_string(path: &Path, root: &Path) -> Result<String> {
    let relative = path.strip_prefix(root)?;
    Ok(relative.to_string_lossy().into_owned())
}

fn source_name(base: SourceBase, unit: &Unit, source_key: &str, relative: &str) -> String {
    let package_name = unit.package_name();
    let hash = stable_hash(&format!(
        "{}\0{}\0{}\0{}\0{}",
        base.label(),
        package_name,
        unit.package_version(),
        source_key,
        relative
    ));
    format!(
        "cargo-unit-source-{}-{}-{hash}",
        store_name_component(package_name.as_ref()),
        store_name_component(unit.package_version())
    )
}

fn store_name_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '-' | '.' | '_' => ch,
            _ => '-',
        })
        .collect();

    if component.is_empty() {
        "unknown".to_string()
    } else {
        component
    }
}

fn stable_hash(value: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    hex16(&digest[..8])
}

fn local_source_key(unit: &Unit) -> String {
    format!("path#{}@{}", unit.package_name(), unit.package_version())
}

fn vendor_source_key(unit: &Unit, cargo_lock_sources: &CargoLockSources) -> Result<String> {
    let source = cargo_lock_sources.source_for_unit(unit)?;
    Ok(format!(
        "{}#{}@{}",
        source,
        unit.package_name(),
        unit.package_version()
    ))
}

fn external_source_from_pkg_id(pkg_id: &str) -> Option<String> {
    if pkg_id.starts_with("registry+")
        || pkg_id.starts_with("git+")
        || pkg_id.starts_with("sparse+")
    {
        let (source, _) = pkg_id.rsplit_once('#')?;
        return Some(source.to_string());
    }

    let (_, rest) = pkg_id.split_once(" (")?;
    let source = rest.strip_suffix(')')?;
    if source.starts_with("registry+")
        || source.starts_with("git+")
        || source.starts_with("sparse+")
    {
        Some(source.to_string())
    } else {
        None
    }
}

fn local_package_root_from_pkg_id(pkg_id: &str) -> Option<PathBuf> {
    if let Some(rest) = pkg_id.strip_prefix("path+file://") {
        let (path, _) = rest.split_once('#')?;
        return percent_decode_path(path).map(PathBuf::from);
    }

    let (_, rest) = pkg_id.split_once("(path+file://")?;
    let (path, _) = rest.split_once(')')?;
    percent_decode_path(path).map(PathBuf::from)
}

fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = hex_value(*bytes.get(index + 1)?)?;
            let lo = hex_value(*bytes.get(index + 2)?)?;
            out.push((hi << 4) | lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(out).ok()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex16(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(16);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn source_closure_relatives(root: &Path, source_boundary: &Path) -> Result<Vec<String>> {
    let source_boundary = normalize_path(source_boundary);
    let mut included_roots = BTreeSet::from([normalize_path(root)]);
    let mut queue = VecDeque::from([normalize_path(root)]);

    while let Some(scan_root) = queue.pop_front() {
        collect_source_closure_roots(
            &scan_root,
            &source_boundary,
            &mut included_roots,
            &mut queue,
        )?;
    }

    included_roots
        .iter()
        .map(|path| relative_path_string(path, &source_boundary))
        .collect()
}

fn collect_source_closure_roots(
    root: &Path,
    source_boundary: &Path,
    included_roots: &mut BTreeSet<PathBuf>,
    queue: &mut VecDeque<PathBuf>,
) -> Result<()> {
    if !root.exists() || !root.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            let target = fs::read_link(&path)?;
            let target = if target.is_absolute() {
                target
            } else {
                path.parent().unwrap_or(root).join(target)
            };

            let target = normalize_path(&target);
            if !target.starts_with(source_boundary) {
                return Err(eyre!(
                    "source symlink {} points outside source boundary {} to {}",
                    path.display(),
                    source_boundary.display(),
                    target.display()
                ));
            }

            if !path_is_covered_by_roots(&target, included_roots) {
                if target.is_dir() {
                    queue.push_back(target.clone());
                }
                included_roots.insert(target);
            }
        } else if file_type.is_dir() {
            collect_source_closure_roots(&path, source_boundary, included_roots, queue)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            scan_rust_includes_into_closure(&path, source_boundary, included_roots, queue);
        }
    }

    Ok(())
}

/// Extend `included_roots` / `queue` with any directories reached through
/// `include!`, `include_bytes!`, or `include_str!` macros in `file` whose
/// argument is a plain or `r"…"` string literal. Paths are resolved
/// relative to the source file's directory and normalized; matches outside
/// `source_boundary` are dropped on the assumption they come from build
/// scripts via `OUT_DIR` or similar (rustc will surface a clear error if
/// the file is genuinely missing). Non-literal arguments such as
/// `concat!(env!("OUT_DIR"), "/x")` cannot be resolved statically and are
/// skipped on purpose.
fn scan_rust_includes_into_closure(
    file: &Path,
    source_boundary: &Path,
    included_roots: &mut BTreeSet<PathBuf>,
    queue: &mut VecDeque<PathBuf>,
) {
    let Ok(source) = fs::read_to_string(file) else {
        return;
    };

    let file_dir = file.parent().unwrap_or(file);
    for include_arg in extract_include_macro_paths(&source) {
        let resolved = normalize_path(&file_dir.join(&include_arg));
        if !resolved.starts_with(source_boundary) {
            continue;
        }
        let Some(include_root) = include_closure_root(&resolved, source_boundary) else {
            continue;
        };
        if !path_is_covered_by_roots(&include_root, included_roots) {
            queue.push_back(include_root.clone());
            included_roots.insert(include_root);
        }
    }
}

fn include_closure_root(resolved: &Path, source_boundary: &Path) -> Option<PathBuf> {
    let include_root = if resolved.is_dir() {
        resolved
    } else {
        resolved.parent().unwrap_or(resolved)
    };

    if include_root == source_boundary {
        // A boundary file such as vendorDir/README.md must stay a file;
        // promoting it to vendorDir walks every vendored crate symlink.
        if resolved == source_boundary {
            return None;
        }
        return Some(resolved.to_path_buf());
    }

    Some(include_root.to_path_buf())
}

/// Lift path arguments out of `include!`, `include_bytes!`, and
/// `include_str!` macro calls. The scan is intentionally textual: it does
/// not parse Rust, so false positives inside comments and string literals
/// are possible but harmless (an extra non-existent directory in the
/// closure is filtered out at the Nix layer). Plain `"…"` and `r"…"`
/// literals are resolved as files. `concat!` arguments with a leading
/// literal directory are resolved to that directory; the computed filename
/// stays dynamic, but the source closure still contains the data tree.
/// Other computed arguments such as `env!`, identifiers, and raw strings
/// with `#` delimiters are skipped.
fn extract_include_macro_paths(source: &str) -> Vec<String> {
    const MARKERS: &[&str] = &["include!", "include_bytes!", "include_str!"];
    let mut paths = Vec::new();
    for marker in MARKERS {
        let mut cursor = 0;
        while let Some(found) = source[cursor..].find(marker) {
            let start = cursor + found;
            let after = start + marker.len();
            cursor = after;
            // Word-boundary check: `my_include_bytes!` should not match.
            if start > 0
                && source[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            let tail = source[after..].trim_start();
            let Some(tail) = tail.strip_prefix('(') else {
                continue;
            };
            let tail = tail.trim_start();
            if let Some((literal, _)) = parse_rust_string_literal(tail) {
                if !literal.is_empty() {
                    paths.push(literal);
                }
                continue;
            }

            let Some(concat_tail) = tail.strip_prefix("concat!") else {
                continue;
            };
            let Some(concat_body) = concat_tail.trim_start().strip_prefix('(') else {
                continue;
            };
            if let Some((literal, _)) = parse_rust_string_literal(concat_body.trim_start())
                && let Some(directory) = literal.strip_suffix('/')
                && !directory.is_empty()
            {
                paths.push(directory.to_string());
            }
        }
    }
    paths
}

fn parse_rust_string_literal(source: &str) -> Option<(String, &str)> {
    let (source, is_raw) = match source.strip_prefix('r') {
        Some(after_r) if after_r.starts_with('"') => (after_r, true),
        _ => (source, false),
    };
    let body = source.strip_prefix('"')?;
    let mut chars = body.char_indices();
    let mut literal = String::new();
    while let Some((index, c)) = chars.next() {
        if c == '"' {
            return Some((literal, &body[index + c.len_utf8()..]));
        }
        if c == '\\' && !is_raw {
            match chars.next().map(|(_, escaped)| escaped) {
                Some('n') => literal.push('\n'),
                Some('t') => literal.push('\t'),
                Some('r') => literal.push('\r'),
                Some('"') => literal.push('"'),
                Some('\'') => literal.push('\''),
                Some('\\') => literal.push('\\'),
                Some(other) => literal.push(other),
                None => break,
            }
        } else {
            literal.push(c);
        }
    }

    None
}

fn path_is_covered_by_roots(path: &Path, roots: &BTreeSet<PathBuf>) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}

fn source_path_expr(source: &SourceEntry, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(&source.root).map_err(|_| {
        eyre!(
            "unit source path {} is outside scoped source root {}",
            path.display(),
            source.root.display()
        )
    })?;
    let relative = relative.to_string_lossy();
    if relative.is_empty() {
        Ok("$src".to_string())
    } else {
        Ok(format!("$src/{relative}"))
    }
}

fn ensure_source_contains_unit(source: &SourceEntry, unit: &Unit) -> Result<()> {
    let path = Path::new(&unit.target.src_path);
    source_path_expr(source, path).map(|_| ())
}

fn render_roots(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    render_unit_refs(&graph.roots, prepared)
}

fn render_unit_refs(roots: &[usize], prepared: &PreparedGraph) -> String {
    roots
        .iter()
        .map(|index| format!("units.{}", nix_attr(&prepared.names[*index])))
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_root_entries_for(
    roots: &[usize],
    units: &[Unit],
    prepared: &PreparedGraph,
    include: impl Fn(&Unit) -> bool,
) -> String {
    let mut entries = String::new();
    let mut seen = BTreeSet::new();
    for index in roots {
        let unit = &units[*index];
        if !include(unit) || !seen.insert(unit.target.name.clone()) {
            continue;
        }
        let _ = writeln!(
            entries,
            "    {} = units.{};",
            nix_attr(&unit.target.name),
            nix_attr(&prepared.names[*index])
        );
    }
    entries
}

fn render_root_entries(
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    include: impl Fn(&Unit) -> bool,
) -> String {
    render_root_entries_for(&graph.roots, &graph.units, prepared, include)
}

fn render_checked_roots(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    graph
        .roots
        .iter()
        .map(|index| {
            format!(
                "withPolicyChecks units.{}",
                nix_attr(&prepared.names[*index])
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_target_sets(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let root_sets = if graph.root_sets.is_empty() {
        vec![graph.roots.clone()]
    } else {
        graph.root_sets.clone()
    };
    let test_keys = compute_test_keys(graph, prepared);
    let benchmark_keys = compute_benchmark_keys(graph, prepared);
    let doctest_keys = compute_doctest_keys(graph, prepared);

    root_sets
        .iter()
        .map(|roots| {
            format!(
                "    {{\n      roots = [ {} ];\n      binaries = {{\n{}      }};\n      libraries = {{\n{}      }};\n      benchmarks = {{\n{}      }};\n      tests = {{\n{}      }};\n      doctests = {{\n{}      }};\n    }}",
                render_unit_refs(roots, prepared),
                render_root_entries_for(roots, &graph.units, prepared, Unit::is_bin),
                render_root_entries_for(roots, &graph.units, prepared, Unit::is_library),
                render_benchmark_entries_for(roots, &graph.units, prepared, &benchmark_keys),
                render_test_entries_for(roots, &graph.units, prepared, &test_keys),
                render_doctest_entries_for(roots, &graph.units, &doctest_keys),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Globally stable key per root runnable unit. Picks `target.name` when no
/// other matching root unit shares it, falling back to the unit-specific name.
fn compute_root_keys(
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    include: impl Fn(&Unit) -> bool,
) -> BTreeMap<usize, String> {
    let mut all_roots: BTreeSet<usize> = graph.roots.iter().copied().collect();
    for set in &graph.root_sets {
        all_roots.extend(set.iter().copied());
    }
    let roots: Vec<usize> = all_roots
        .into_iter()
        .filter(|index| include(&graph.units[*index]))
        .collect();

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for index in &roots {
        *counts
            .entry(graph.units[*index].target.name.as_str())
            .or_insert(0) += 1;
    }

    let mut keys = BTreeMap::new();
    for index in roots {
        let unit = &graph.units[index];
        let key = if counts[unit.target.name.as_str()] == 1 {
            unit.target.name.clone()
        } else {
            prepared.names[index].clone()
        };
        keys.insert(index, key);
    }
    keys
}

fn compute_test_keys(graph: &UnitGraph, prepared: &PreparedGraph) -> BTreeMap<usize, String> {
    compute_root_keys(graph, prepared, Unit::is_test)
}

fn compute_benchmark_keys(graph: &UnitGraph, prepared: &PreparedGraph) -> BTreeMap<usize, String> {
    compute_root_keys(graph, prepared, Unit::is_benchmark)
}

fn test_binary_expr(unit: &Unit, prepared: &PreparedGraph, index: usize) -> String {
    let unit_ref = format!("${{units.{}}}", nix_attr(&prepared.names[index]));
    format!("{unit_ref}/bin/{}", unit.target.name)
}

fn render_doctest_command(
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    index: usize,
    mode: DoctestCommandMode,
) -> Result<String> {
    let unit = &graph.units[index];
    let source = prepared.source_entry(index)?;
    let package_root = source_path_expr(source, &crate_root_for_unit(unit))?;
    let source_path = source_path_expr(source, Path::new(&unit.target.src_path))?;
    let unit_ref = format!("${{units.{}}}", nix_attr(&prepared.names[index]));
    let mut script = String::new();

    script.push_str("set -euo pipefail\n");
    writeln!(script, "export src=\"${{{}}}\"", prepared.source_ref(index))?;
    writeln!(
        script,
        "export CARGO_MANIFEST_DIR={}",
        shell::double_quote(&package_root)
    )?;
    script.push_str("cd \"$CARGO_MANIFEST_DIR\"\n");
    script.push_str(&cargo_package_exports(unit)?);
    script.push_str("build_script_rustdoc_args=()\n");
    script.push_str("doctest_build_args=()\n");
    script.push_str("doctest_runtime_library_paths=()\n");
    script.push_str("rustdoc_args=( --test )\n");
    match mode {
        DoctestCommandMode::List => {
            script.push_str("rustdoc_args+=( -Z unstable-options --output-format doctest )\n");
        }
        DoctestCommandMode::RunAll => {}
        DoctestCommandMode::RunCase => {
            script.push_str("rustdoc_args+=( --test-args \"$TEST_NAME\" --test-args --include-ignored --test-args --nocapture )\n");
        }
    }
    push_rustdoc_arg(&mut script, "--crate-name");
    push_rustdoc_arg(&mut script, &unit.target.name.replace('-', "_"));
    push_rustdoc_arg(&mut script, "--edition");
    push_rustdoc_arg(&mut script, &unit.target.edition);
    for rustflag in &unit.lint_rustflags {
        push_rustdoc_arg(&mut script, rustflag);
    }
    for arg in &unit.check_cfg_args {
        push_rustdoc_arg(&mut script, arg);
    }
    for feature in &unit.features {
        push_rustdoc_arg(&mut script, "--cfg");
        push_rustdoc_arg(&mut script, &format!("feature=\"{feature}\""));
    }
    if let Some(platform) = &unit.platform {
        push_rustdoc_arg(&mut script, "--target");
        push_rustdoc_arg(&mut script, platform);
    }
    append_doctest_builder_args(&mut script, graph, prepared, index, mode);
    for dep_index in &prepared.transitive_unit_deps[index] {
        let dep = &graph.units[*dep_index];
        if dep.is_bin() {
            continue;
        }
        writeln!(
            script,
            "rustdoc_args+=( -L \"dependency=${{units.{}}}/lib\" )",
            nix_attr(&prepared.names[*dep_index])
        )?;
    }
    writeln!(script, "rustdoc_args+=( -L \"dependency={unit_ref}/lib\" )")?;
    writeln!(
        script,
        "rustdoc_args+=( --extern \"{}=$(cat {unit_ref}/nix-support/extern-path)\" )",
        unit.target.name.replace('-', "_")
    )?;
    for dependency in &unit.dependencies {
        let dep_unit = &graph.units[dependency.index];
        if dep_unit.is_run_custom_build() || dep_unit.is_bin() {
            continue;
        }
        writeln!(
            script,
            "rustdoc_args+=( --extern \"{}=$(cat ${{units.{}}}/nix-support/extern-path)\" )",
            dependency.extern_crate_name,
            nix_attr(&prepared.names[dependency.index])
        )?;
    }
    writeln!(
        script,
        "rustdoc_args+=( {} )",
        shell::double_quote(&source_path)
    )?;
    script.push_str("set -x\n");
    match mode {
        DoctestCommandMode::RunCase => {
            script.push_str("doctest_log=$(mktemp)\n");
            script.push_str("rustdoc \"''${rustdoc_args[@]}\" 2>&1 | tee \"$doctest_log\"\n");
            script.push_str("if ! grep -q '^running 1 test$' \"$doctest_log\"; then\n");
            script.push_str(
                "  echo \"rustdoc filter did not run exactly one doctest: $TEST_NAME\" >&2\n",
            );
            script.push_str("  exit 1\n");
            script.push_str("fi\n");
        }
        DoctestCommandMode::List | DoctestCommandMode::RunAll => {
            script.push_str("rustdoc \"''${rustdoc_args[@]}\"\n");
        }
    }
    Ok(script)
}

#[derive(Clone, Copy)]
enum DoctestCommandMode {
    List,
    RunAll,
    RunCase,
}

fn push_rustdoc_arg(script: &mut String, value: &str) {
    let _ = writeln!(script, "rustdoc_args+=( {} )", shell::quote(value));
}

fn append_doctest_builder_args(
    script: &mut String,
    graph: &UnitGraph,
    prepared: &PreparedGraph,
    index: usize,
    mode: DoctestCommandMode,
) {
    let unit = &graph.units[index];
    for rustflag in &unit.profile.rustflags {
        push_doctest_build_arg(script, rustflag);
    }
    if let Some(run_index) = unit_build_script_run(graph, index) {
        let run_ref = format!("${{units.{}}}", nix_attr(&prepared.names[run_index]));
        append_doctest_build_script_flag_reader(script, &run_ref);
    }

    script.push_str("rustdoc_args+=( \"''${build_script_rustdoc_args[@]}\" )\n");
    if !matches!(mode, DoctestCommandMode::List) {
        script.push_str("if [ \"''${#doctest_build_args[@]}\" -gt 0 ]; then\n");
        script.push_str("  rustdoc_args+=( -Z unstable-options )\n");
        script.push_str("fi\n");
    }
    script.push_str("for doctest_build_arg in \"''${doctest_build_args[@]}\"; do\n");
    script.push_str("  rustdoc_args+=( --doctest-build-arg \"$doctest_build_arg\" )\n");
    script.push_str("done\n");
    script.push_str("if [ \"''${#doctest_runtime_library_paths[@]}\" -gt 0 ]; then\n");
    script.push_str(
        "  doctest_runtime_library_path=$(IFS=:; printf '%s' \"''${doctest_runtime_library_paths[*]}\")\n",
    );
    script.push_str(
        "  doctest_runtime_library_path_host=$(rustc -vV | sed -n 's/^host: //p')\n",
    );
    script.push_str("  case \"$doctest_runtime_library_path_host\" in\n");
    script.push_str(
        "    *apple-darwin*)\n      doctest_runtime_library_path_var=DYLD_FALLBACK_LIBRARY_PATH\n      doctest_runtime_library_path_default=\"$HOME/lib:/usr/local/lib:/usr/lib\"\n      ;;\n",
    );
    script.push_str(
        "    *)\n      doctest_runtime_library_path_var=LD_LIBRARY_PATH\n      doctest_runtime_library_path_default=\n      ;;\n",
    );
    script.push_str("  esac\n");
    script.push_str(
        "  doctest_runtime_library_path_current=\"''${!doctest_runtime_library_path_var-}\"\n",
    );
    script.push_str("  if [ -n \"$doctest_runtime_library_path_current\" ]; then\n");
    script.push_str(
        "    export \"$doctest_runtime_library_path_var=$doctest_runtime_library_path:$doctest_runtime_library_path_current\"\n",
    );
    script.push_str("  else\n");
    script.push_str("    if [ -n \"$doctest_runtime_library_path_default\" ]; then\n");
    script.push_str(
        "      export \"$doctest_runtime_library_path_var=$doctest_runtime_library_path:$doctest_runtime_library_path_default\"\n",
    );
    script.push_str("    else\n");
    script.push_str(
        "      export \"$doctest_runtime_library_path_var=$doctest_runtime_library_path\"\n",
    );
    script.push_str("    fi\n");
    script.push_str("  fi\n");
    script.push_str("fi\n");
}

fn push_doctest_build_arg(script: &mut String, value: &str) {
    let _ = writeln!(script, "doctest_build_args+=( {} )", shell::quote(value));
}

fn append_doctest_build_script_flag_reader(script: &mut String, run_ref: &str) {
    let quoted_run_ref = format!("\"{run_ref}\"");

    script.push('\n');
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/rustc-cfg ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && build_script_rustdoc_args+=( '--cfg' \"$line\" )\n  done < {quoted_run_ref}/rustc-cfg\nfi",
    );
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/rustc-link-search ]; then\n  while IFS= read -r line; do\n    if [ -n \"$line\" ]; then\n      build_script_rustdoc_args+=( '-L' \"$line\" )\n      link_search_path=\"$line\"\n      case \"$link_search_path\" in\n        *=*) link_search_path=\"''${{link_search_path#*=}}\" ;;\n      esac\n      case \"$link_search_path\" in\n        {quoted_run_ref}/out-dir|{quoted_run_ref}/out-dir/*) doctest_runtime_library_paths+=( \"$link_search_path\" ) ;;\n      esac\n    fi\n  done < {quoted_run_ref}/rustc-link-search\nfi",
    );
    append_doctest_link_arg_reader(script, &quoted_run_ref, "rustc-link-arg");
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/rustc-env ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && export \"$line\"\n  done < {quoted_run_ref}/rustc-env\nfi",
    );
    let _ = writeln!(script, "export OUT_DIR={quoted_run_ref}/out-dir\n");
}

fn append_doctest_link_arg_reader(script: &mut String, quoted_run_ref: &str, file: &str) {
    let _ = writeln!(
        script,
        "if [ -f {quoted_run_ref}/{file} ]; then\n  while IFS= read -r line; do\n    [ -n \"$line\" ] && doctest_build_args+=( -C \"link-arg=$line\" )\n  done < {quoted_run_ref}/{file}\nfi",
    );
}

fn render_benchmark_entries_for(
    roots: &[usize],
    units: &[Unit],
    prepared: &PreparedGraph,
    keys: &BTreeMap<usize, String>,
) -> String {
    let mut entries = String::new();
    let mut seen = BTreeSet::new();
    for index in roots {
        let unit = &units[*index];
        if !unit.is_benchmark() {
            continue;
        }
        let key = keys
            .get(index)
            .expect("compute_benchmark_keys covers every root benchmark unit")
            .clone();
        if !seen.insert(key.clone()) {
            continue;
        }
        let _ = writeln!(
            entries,
            "    {} = units.{};",
            nix_attr(&key),
            nix_attr(&prepared.names[*index])
        );
    }

    entries
}

fn render_benchmark_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let keys = compute_benchmark_keys(graph, prepared);
    render_benchmark_entries_for(&graph.roots, &graph.units, prepared, &keys)
}

fn render_test_entries_for(
    roots: &[usize],
    units: &[Unit],
    prepared: &PreparedGraph,
    keys: &BTreeMap<usize, String>,
) -> String {
    let mut entries = String::new();
    let mut seen = BTreeSet::new();
    for index in roots {
        let unit = &units[*index];
        if !unit.is_test() {
            continue;
        }
        let key = keys
            .get(index)
            .expect("compute_test_keys covers every root test unit")
            .clone();
        if !seen.insert(key.clone()) {
            continue;
        }
        let binary = test_binary_expr(unit, prepared, *index);
        let _ = writeln!(
            entries,
            "    {} = mkTestEntry {{ name = {}; binary = \"{binary}\"; packageName = {}; }};",
            nix_attr(&key),
            nix_attr(&key),
            nix_attr(&unit.package_name()),
        );
    }

    entries
}

fn render_test_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let keys = compute_test_keys(graph, prepared);
    render_test_entries_for(&graph.roots, &graph.units, prepared, &keys)
}

fn compute_doctest_keys(graph: &UnitGraph, prepared: &PreparedGraph) -> BTreeMap<usize, String> {
    compute_root_keys(graph, prepared, Unit::has_doctests)
}

fn render_doctest_entries_for(
    roots: &[usize],
    units: &[Unit],
    keys: &BTreeMap<usize, String>,
) -> String {
    let mut entries = String::new();
    let mut seen = BTreeSet::new();
    for index in roots {
        let unit = &units[*index];
        if !unit.has_doctests() {
            continue;
        }
        let key = keys
            .get(index)
            .expect("compute_doctest_keys covers every root doctest unit")
            .clone();
        if !seen.insert(key.clone()) {
            continue;
        }
        let _ = writeln!(
            entries,
            "    {} = mkDoctestEntry (builtins.head (builtins.filter (target: target.name == {}) doctestTargets));",
            nix_attr(&key),
            nix_attr(&key),
        );
    }

    entries
}

fn render_doctest_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let keys = compute_doctest_keys(graph, prepared);
    render_doctest_entries_for(
        &keys.keys().copied().collect::<Vec<_>>(),
        &graph.units,
        &keys,
    )
}
/// One `{ name; binary; }` per unique test target across every root set.
/// The template feeds this into a single manifest derivation so test
/// enumeration is one IFD instead of one per binary.
fn render_test_target_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let keys = compute_test_keys(graph, prepared);
    let mut by_key: BTreeMap<String, String> = BTreeMap::new();
    for (&index, key) in &keys {
        let unit = &graph.units[index];
        if by_key.contains_key(key) {
            continue;
        }
        let source = prepared
            .source_entry(index)
            .expect("prepared graph has source entries for every test target");
        let package_root = if source.relative.is_empty() {
            "."
        } else {
            source.relative.as_str()
        };
        by_key.insert(
            key.clone(),
            format!(
                "{{ name = {}; binary = \"{}\"; packageName = {}; packageVersion = {}; packageRoot = {}; sourceStoreName = {}; }}",
                nix_attr(key),
                test_binary_expr(unit, prepared, index),
                nix_attr(&unit.package_name()),
                nix_attr(unit.package_version()),
                nix_attr(package_root),
                nix_attr(&source.name),
            ),
        );
    }
    let mut entries = String::new();
    for (_key, target) in by_key {
        let _ = writeln!(entries, "    {target}");
    }
    entries
}

fn render_doctest_target_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> Result<String> {
    let keys = compute_doctest_keys(graph, prepared);
    let mut by_key: BTreeMap<String, String> = BTreeMap::new();
    for (&index, key) in &keys {
        if by_key.contains_key(key) {
            continue;
        }
        let unit = &graph.units[index];
        let source = prepared
            .source_entry(index)
            .expect("prepared graph has source entries for every doctest target");
        let package_root = if source.relative.is_empty() {
            "."
        } else {
            source.relative.as_str()
        };
        by_key.insert(
            key.clone(),
            format!(
                "{{ name = {}; packageName = {}; packageVersion = {}; packageRoot = {}; sourceStoreName = {}; listCommand = {}; allCommand = {}; runCommand = {}; }}",
                nix_attr(key),
                nix_attr(&unit.package_name()),
                nix_attr(unit.package_version()),
                nix_attr(package_root),
                nix_attr(&source.name),
                nix_indented_string(&render_doctest_command(
                    graph,
                    prepared,
                    index,
                    DoctestCommandMode::List,
                )?),
                nix_indented_string(&render_doctest_command(
                    graph,
                    prepared,
                    index,
                    DoctestCommandMode::RunAll,
                )?),
                nix_indented_string(&render_doctest_command(
                    graph,
                    prepared,
                    index,
                    DoctestCommandMode::RunCase,
                )?),
            ),
        );
    }
    let mut entries = String::new();
    for (_key, target) in by_key {
        let _ = writeln!(entries, "    {target}");
    }
    Ok(entries)
}

/// One `{ name; binary; }` per unique benchmark target across every root set.
/// The template feeds this into benchmark plans and previous-vs-next Tango
/// comparisons without another Cargo metadata pass.
fn render_benchmark_target_entries(graph: &UnitGraph, prepared: &PreparedGraph) -> String {
    let keys = compute_benchmark_keys(graph, prepared);
    let mut by_key: BTreeMap<String, String> = BTreeMap::new();
    for (&index, key) in &keys {
        let unit = &graph.units[index];
        if by_key.contains_key(key) {
            continue;
        }
        let source = prepared
            .source_entry(index)
            .expect("prepared graph has source entries for every benchmark target");
        let package_root = if source.relative.is_empty() {
            "."
        } else {
            source.relative.as_str()
        };
        by_key.insert(
            key.clone(),
            format!(
                "{{ name = {}; binary = \"{}\"; packageName = {}; packageVersion = {}; packageRoot = {}; sourceStoreName = {}; }}",
                nix_attr(key),
                test_binary_expr(unit, prepared, index),
                nix_attr(&unit.package_name()),
                nix_attr(unit.package_version()),
                nix_attr(package_root),
                nix_attr(&source.name),
            ),
        );
    }
    let mut entries = String::new();
    for (_key, target) in by_key {
        let _ = writeln!(entries, "    {target}");
    }
    entries
}

fn nix_attr(value: &str) -> String {
    serde_json::to_string(value).expect("serialize Nix string")
}

fn nix_string_list(values: &[String]) -> String {
    format!(
        "[ {} ]",
        values
            .iter()
            .map(|value| nix_attr(value))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

struct Attrs {
    values: Vec<(String, String)>,
}

impl Attrs {
    const fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn string(&mut self, name: &str, value: &str) {
        self.values
            .push((name.to_string(), format!("{};", nix_attr(value))));
    }

    fn bool(&mut self, name: &str, value: bool) {
        self.values.push((
            name.to_string(),
            format!("{};", if value { "true" } else { "false" }),
        ));
    }

    fn expr(&mut self, name: &str, value: &str) {
        self.values.push((name.to_string(), format!("{value};")));
    }

    fn multiline(&mut self, name: &str, value: &str) {
        self.values
            .push((name.to_string(), format!("''\n{value}  '';")));
    }

    fn render(self) -> String {
        let mut out = String::new();
        out.push_str("{\n");
        for (name, value) in self.values {
            let _ = writeln!(out, "      {name} = {value}");
        }
        out.push_str("    }");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo_lock_sources(packages: &[(&str, &str, &str)]) -> CargoLockSources {
        CargoLockSources {
            packages: packages
                .iter()
                .map(|(name, version, source)| CargoLockPackage {
                    name: (*name).to_string(),
                    version: (*version).to_string(),
                    source: (*source).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn renders_one_derivation_per_build_unit() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: Some("rustc-test".to_string()),
                deny_unused_crate_dependencies: true,
            },
        )
        .unwrap();

        assert!(rendered.contains("units = rec"));
        assert!(rendered.contains("--crate-name"));
        assert!(rendered.contains("sources = {"));
        assert!(rendered.contains("scopedWorkspaceSource \"cargo-unit-source-hello-0.1.0-"));
        assert!(rendered.contains("\"\""));
        assert!(rendered.contains("src = sources."));
        assert!(rendered.contains("\"$src/src/main.rs\""));
        assert!(rendered.contains("default = withPolicyChecks units."));
        assert!(rendered.contains("policyChecks"));
        assert!(rendered.contains("extraRustcArgs"));
        assert!(rendered.contains("tests ="));
        assert!(rendered.contains("--json=unused-externs-silent"));
        assert!(rendered.contains("withPolicyChecks"));
        // Per-unit clippy: the same local unit gets a sibling clippy-driver
        // derivation in `clippyUnits`, threaded through `policyChecks.clippy`.
        assert!(rendered.contains("clippyUnits = rec"));
        assert!(rendered.contains("mkClippyUnit"));
        assert!(rendered.contains("env \"''${rustc_env[@]}\" clippy-driver"));
        assert!(rendered.contains("extraClippyLintArgs"));
        assert!(rendered.contains("clippy = clippyPolicyAggregate;"));
        assert!(rendered.contains("clippyPolicyAggregate ="));
    }

    #[test]
    fn external_only_graph_emits_no_clippy_units() {
        // Vendored crates compile under `--cap-lints warn` and aren't owned by
        // the workspace, so they should never produce per-unit clippy
        // derivations. A graph with only external units must skip clippy
        // entirely and keep `policyChecks` free of a `clippy` entry.
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "serde",
                    "src_path": "/vendor/serde/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[(
                    "serde",
                    "1.0.0",
                    "registry+https://github.com/rust-lang/crates.io-index",
                )]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        // `clippyUnits = rec { };` rendered empty proves no per-unit clippy
        // derivations were emitted. The `clippy = clippyUnits;` text and
        // the template's `mkClippyUnit` helper are template-literal and
        // always present; the driver invocation only appears inside a
        // rendered clippy unit's build phase, so it's the load-bearing tell.
        assert!(rendered.contains("clippyUnits = rec {\n  };"));
        assert!(!rendered.contains("env \"''${rustc_env[@]}\" clippy-driver"));
        assert!(!rendered.contains("mkClippyUnit {\n      pname ="));
    }

    #[test]
    fn exposes_test_roots_as_runnable_checks() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "hello",
                    "src_path": "/workspace/src/lib.rs",
                    "edition": "2024",
                    "test": true
                  },
                  "profile": { "name": "test", "opt_level": "0" },
                  "features": [],
                  "mode": "test",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("tests = {"));
        assert!(rendered.contains("\"hello\" = mkTestEntry { name = \"hello\";"));
        assert!(rendered.contains("packageName = \"hello\";"));
        assert!(rendered.contains("/bin/hello\";"));
        assert!(rendered.contains("testRunPrelude ? \"\""));
        assert!(rendered.contains("testArgsByPackage ? {}"));
        assert!(rendered.contains("mkTestEntry ="));
        assert!(rendered.contains("RUST_TEST_THREADS"));
        assert!(rendered.contains("mkTestCases ="));
        assert!(rendered.contains("testTargets = ["));
        assert!(rendered.contains("testTargetNamesByPackage ="));
        assert!(rendered.contains("pkgs.lib.groupBy (target: target.packageName) testTargets"));
        assert!(rendered.contains("doctestTargetNamesByPackage ="));
        assert!(rendered.contains("{ name = \"hello\"; binary ="));
        assert!(!rendered.contains("units.\\\""));
        assert!(rendered.contains("packageName = \"hello\";"));
        assert!(rendered.contains("packageRoot = \".\";"));
        assert!(rendered.contains("sourceStoreName = \"cargo-unit-source-hello-0.1.0-"));
        assert!(rendered.contains("sourcePackageRoot ="));
        assert!(rendered.contains("test_cwd="));
        assert!(rendered.contains("cd \"$test_cwd\""));
        assert!(rendered.contains("testPlan = mkTestPlan \"cargo-unit-test-plan\";"));
        assert!(rendered.contains("coverageReport = mkCoverageReport {};"));
        assert!(rendered.contains("makeCoverageReport = mkCoverageReport;"));
        assert!(rendered.contains("writableTestCwd ? true"));
        assert!(rendered.contains(".cargo-unit-writable-cwd-ready"));
        assert!(rendered.contains("llvm-profdata"));
        assert!(rendered.contains("llvm-cov"));
        assert!(!rendered.contains("fallbackLlvmCov"));
        assert!(rendered.contains("source-roots.tsv"));
        assert!(rendered.contains("testManifestDrv ="));
        assert!(rendered.contains("cargo-unit-test-manifest"));
    }

    #[test]
    fn bin_exe_env_uses_build_bins_for_integration_tests() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::create_dir_all(workspace.path().join("tests")).unwrap();
        fs::write(
            workspace.path().join("Cargo.toml"),
            r#"[package]
name = "dag-runner"
version = "0.1.0"
"#,
        )
        .unwrap();
        let main_rs = workspace.path().join("src/main.rs");
        let integration_rs = workspace.path().join("tests/integration.rs");
        fs::write(&main_rs, "fn main() {}\n").unwrap();
        fs::write(&integration_rs, "#[test]\nfn runs() {}\n").unwrap();
        let main_rs = main_rs.display().to_string();
        let integration_rs = integration_rs.display().to_string();
        let pkg_id = format!(
            "path+file://{}#dag-runner@0.1.0",
            workspace.path().display()
        );
        let bin_target = serde_json::json!({
          "kind": ["bin"],
          "crate_types": ["bin"],
          "name": "dag-runner",
          "src_path": main_rs,
          "edition": "2024"
        });

        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
          "version": 1,
          "units": [
            {
              "pkg_id": pkg_id,
              "target": bin_target,
              "profile": { "name": "release", "opt_level": "3", "rustflags": ["-C", "target-feature=+sse2"] },
              "features": [],
              "mode": "build",
              "dependencies": []
            },
            {
              "pkg_id": pkg_id,
              "target": bin_target,
              "profile": { "name": "test", "opt_level": "0" },
              "features": [],
              "mode": "test",
              "dependencies": []
            },
            {
              "pkg_id": pkg_id,
              "target": {
                "kind": ["test"],
                "crate_types": ["bin"],
                "name": "integration",
                "src_path": integration_rs,
                "edition": "2024"
              },
              "profile": { "name": "test", "opt_level": "0" },
              "features": [],
              "mode": "test",
              "dependencies": [
                { "index": 0, "extern_crate_name": "dag_runner" }
              ]
            },
            {
              "pkg_id": pkg_id,
              "target": bin_target,
              "profile": { "name": "dev", "opt_level": "0" },
              "features": ["extra"],
              "mode": "build",
              "dependencies": []
            }
          ],
          "roots": [0, 1, 2]
        }))
        .unwrap();

        assert!(same_package_bins(&graph, 1).is_empty());
        assert_eq!(
            same_package_bins(&graph, 2),
            vec![("dag-runner".to_string(), 0)]
        );

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.path().to_path_buf(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        // The build unit (rustc) and its sibling clippy unit each set
        // CARGO_BIN_EXE_<name> for integration tests because clippy needs the
        // same compilation env as rustc. The count rises with the number of
        // unit kinds that lint the integration test target.
        assert_eq!(rendered.matches("CARGO_BIN_EXE_dag-runner=").count(), 2);
        assert!(rendered.contains("rustc_env+=( 'CARGO_BIN_EXE_dag-runner=${units."));
        assert!(!rendered.contains("export CARGO_BIN_EXE_dag-runner"));
        assert!(rendered.contains("env \"''${rustc_env[@]}\" rustc \"''${rustc_args[@]}\""));
        assert!(rendered.contains("env \"''${rustc_env[@]}\" clippy-driver \"''${rustc_args[@]}\""));
    }

    #[test]
    fn exposes_benchmark_roots_as_tango_comparison_inputs() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "hello",
                    "src_path": "/workspace/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "bench", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bench"],
                    "crate_types": ["bin"],
                    "name": "greeting",
                    "src_path": "/workspace/benches/greeting.rs",
                    "edition": "2024",
                    "test": false
                  },
                  "profile": { "name": "bench", "opt_level": "3" },
                  "features": [],
                  "mode": "test",
                  "dependencies": [
                    { "index": 0, "extern_crate_name": "hello" }
                  ]
                }
              ],
              "roots": [1]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("benchmarks = {"));
        assert!(rendered.contains("\"greeting\" = units."));
        assert!(rendered.contains("benchmarkTargets = ["));
        assert!(rendered.contains("{ name = \"greeting\"; binary ="));
        assert!(
            rendered.contains("benchmarkPlan = mkBenchmarkPlan \"cargo-unit-benchmark-plan\";")
        );
        assert!(rendered.contains("compareTangoBenchmarks = mkTangoBenchmarkComparison;"));
        assert!(rendered.contains("targetSets = ["));
        assert!(!rendered.contains("\"greeting\" = mkTestEntry"));
        assert!(!rendered.contains("rustc_args+=( '--test' )"));
        assert!(rendered.contains("rustc_args+=( '--cfg' )"));
        assert!(rendered.contains("rustc_args+=( 'test' )"));
    }

    #[test]
    fn exposes_root_sets_for_namespaced_target_outputs() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": [],
                  "platform": "x86_64-unknown-linux-musl"
                },
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": [],
                  "platform": "aarch64-apple-darwin"
                }
              ],
              "roots": [0, 1],
              "root_sets": [[0], [1]]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("targetSets = ["));
        assert_eq!(rendered.matches("binaries = {").count(), 3);
        assert_eq!(rendered.matches("\"hello\" = units.").count(), 4);
    }

    #[test]
    fn scopes_doctests_to_each_root_set() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace/alpha#alpha@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "alpha",
                    "src_path": "/workspace/alpha/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "path+file:///workspace/beta#beta@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "beta",
                    "src_path": "/workspace/beta/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "features": [],
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0, 1],
              "root_sets": [[0], [1]]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert_eq!(rendered.matches("\"alpha\" = mkDoctestEntry").count(), 2);
        assert_eq!(rendered.matches("\"beta\" = mkDoctestEntry").count(), 2);
    }

    #[test]
    fn doctest_commands_match_cargo_rustdoc_contract() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("Cargo.toml"),
            r#"[package]
name = "native"
version = "0.1.0"
"#,
        )
        .unwrap();
        let build_rs = workspace.path().join("build.rs");
        let lib_rs = workspace.path().join("src/lib.rs");
        fs::write(&build_rs, "fn main() {}\n").unwrap();
        fs::write(&lib_rs, "pub fn native() {}\n").unwrap();
        let build_rs = build_rs.to_string_lossy();
        let lib_rs = lib_rs.to_string_lossy();
        let pkg_id = format!("path+file://{}#native@0.1.0", workspace.path().display());

        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
          "version": 1,
          "units": [
            {
              "pkg_id": pkg_id,
              "target": {
                "kind": ["custom-build"],
                "crate_types": ["bin"],
                "name": "build-script-build",
                "src_path": build_rs,
                "edition": "2024"
              },
              "profile": { "name": "release", "opt_level": "3" },
              "features": [],
              "mode": "build",
              "dependencies": []
            },
            {
              "pkg_id": pkg_id,
              "target": {
                "kind": ["custom-build"],
                "crate_types": ["bin"],
                "name": "build-script-build",
                "src_path": build_rs,
                "edition": "2024"
              },
              "profile": { "name": "release", "opt_level": "3" },
              "features": [],
              "mode": "run-custom-build",
              "dependencies": [
                { "index": 0, "extern_crate_name": "build_script_build" }
              ]
            },
            {
              "pkg_id": pkg_id,
              "target": {
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "native",
                "src_path": lib_rs,
                "edition": "2024"
              },
              "profile": { "name": "release", "opt_level": "3", "rustflags": ["-C", "target-feature=+sse2"] },
              "lint_rustflags": ["--deny=warnings", "--warn=unexpected_cfgs"],
              "check_cfg_args": ["--check-cfg", "cfg(docsrs,test)"],
              "features": [],
              "mode": "build",
              "dependencies": [
                { "index": 1, "extern_crate_name": "build_script_build" }
              ]
            }
          ],
          "roots": [2]
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.path().to_path_buf(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert_doctest_rendered_contract(&rendered);
    }

    fn assert_doctest_rendered_contract(rendered: &str) {
        assert_eq!(rendered.matches("-Z unstable-options").count(), 3);
        assert!(rendered.contains("build_script_rustdoc_args=()"));
        assert!(rendered.contains("doctest_build_args=()"));
        assert!(rendered.contains("doctest_runtime_library_paths=()"));
        assert!(rendered.contains("rustdoc_args+=( \"''${build_script_rustdoc_args[@]}\" )"));
        assert!(rendered.contains("doctest_build_args+=( '-C' )"));
        assert!(rendered.contains("rustdoc_args+=( '--deny=warnings' )"));
        assert!(rendered.contains("rustdoc_args+=( '--warn=unexpected_cfgs' )"));
        assert!(rendered.contains("rustdoc_args+=( '--check-cfg' )"));
        assert!(rendered.contains("rustdoc_args+=( 'cfg(docsrs,test)' )"));
        assert!(rendered.contains("rustdoc_args+=( --doctest-build-arg \"$doctest_build_arg\" )"));
        assert!(rendered.contains("case \"$link_search_path\" in"));
        assert!(rendered.contains(
            "/out-dir|\"${units."
        ));
        assert!(rendered.contains(
            "/out-dir/*) doctest_runtime_library_paths+=( \"$link_search_path\" ) ;;"
        ));
        assert!(!rendered.contains(
            "[ -n \"$link_search_path\" ] && doctest_runtime_library_paths+=( \"$link_search_path\" )"
        ));
        assert!(rendered.contains(
            "doctest_runtime_library_path_host=$(rustc -vV | sed -n 's/^host: //p')"
        ));
        assert!(rendered.contains(
            "doctest_runtime_library_path_var=DYLD_FALLBACK_LIBRARY_PATH"
        ));
        assert!(rendered.contains(
            "doctest_runtime_library_path_default=\"$HOME/lib:/usr/local/lib:/usr/lib\""
        ));
        assert!(rendered.contains("doctest_runtime_library_path_var=LD_LIBRARY_PATH"));
        assert!(rendered.contains(
            "doctest_runtime_library_path_current=\"''${!doctest_runtime_library_path_var-}\""
        ));
        assert!(rendered.contains(
            "export \"$doctest_runtime_library_path_var=$doctest_runtime_library_path"
        ));
        assert!(!rendered.contains("done < \"${units.native-run}/rustc-link-lib"));
        assert!(!rendered.contains(
            "doctest_build_args+=( -C \"link-arg=$line\" )\n  done < \"${units.native-run}/rustc-cdylib-link-arg"
        ));
        assert!(!rendered.contains("rustdoc_args+=( \"''${build_script_flags[@]}\" )"));
        assert!(rendered.contains("done < \"${units."));
        assert!(rendered.contains("/rustc-env"));
        assert!(rendered.contains("export OUT_DIR=\"${units."));
        assert!(!rendered.contains("--test-args --exact"));
        assert!(rendered.contains("--test-args --include-ignored"));
        assert!(rendered.contains("^running 1 test$"));
    }

    #[test]
    fn aggregates_unused_crate_dependency_reports_by_package() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "serde",
                    "src_path": "/vendor/serde/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "hello",
                    "src_path": "/workspace/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": [
                    { "index": 0, "extern_crate_name": "serde" }
                  ]
                },
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": [
                    { "index": 0, "extern_crate_name": "serde" }
                  ]
                }
              ],
              "roots": [1, 2]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[(
                    "serde",
                    "1.0.0",
                    "registry+https://github.com/rust-lang/crates.io-index",
                )]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: true,
            },
        )
        .unwrap();

        assert_eq!(
            rendered
                .matches("check_unused 'hello 0.1.0' 'serde'")
                .count(),
            1
        );
        assert!(rendered.contains("$out/nix-support/unused-crate-dependencies"));
    }

    #[test]
    fn scopes_local_and_vendor_sources_per_package() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.15",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "itoa",
                    "src_path": "/vendor/itoa-1.0.15/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "path+file:///workspace/crates/core#scope-core@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "scope_core",
                    "src_path": "/workspace/crates/core/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": [
                    { "index": 0, "extern_crate_name": "itoa" }
                  ]
                },
                {
                  "pkg_id": "path+file:///workspace/crates/cli#scope-cli@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "scope_cli",
                    "src_path": "/workspace/crates/cli/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": [
                    { "index": 1, "extern_crate_name": "scope_core" }
                  ]
                }
              ],
              "roots": [2]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[(
                    "itoa",
                    "1.0.15",
                    "registry+https://github.com/rust-lang/crates.io-index",
                )]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("scopedWorkspaceSource \"cargo-unit-source-scope-core-0.1.0-"));
        assert!(rendered.contains("\"crates/core\""));
        assert!(rendered.contains("scopedWorkspaceSource \"cargo-unit-source-scope-cli-0.1.0-"));
        assert!(rendered.contains("\"crates/cli\""));
        assert!(rendered.contains(
            "vendorSources.\"registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.15\""
        ));
        assert!(rendered.contains("sourceAudit = {"));
        assert!(rendered.contains("base = \"vendor-package\";"));
        assert!(rendered.contains("\"$src/src/lib.rs\""));
        assert!(rendered.contains("\"$src/src/main.rs\""));
        assert!(!rendered.contains("${src}/crates/core"));
        assert!(!rendered.contains("${src}/crates/cli"));
        assert!(!rendered.contains("${vendorDir}/itoa-1.0.15"));
    }

    #[test]
    fn vendor_sources_are_keyed_by_full_package_identity() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.15",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "itoa",
                    "src_path": "/vendor/crates-io-itoa/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "sparse+https://example.invalid/index/#itoa@1.0.15",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "itoa",
                    "src_path": "/vendor/example-itoa/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0, 1]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[
                    (
                        "itoa",
                        "1.0.15",
                        "registry+https://github.com/rust-lang/crates.io-index",
                    ),
                    ("itoa", "1.0.15", "sparse+https://example.invalid/index/"),
                ]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains(
            "vendorSources.\"registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.15\""
        ));
        assert!(
            rendered
                .contains("vendorSources.\"sparse+https://example.invalid/index/#itoa@1.0.15\"")
        );
    }

    #[test]
    fn git_vendor_sources_use_locked_source_identity() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "git+https://github.com/shepmaster/snafu.git#snafu@0.9.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "snafu",
                    "src_path": "/vendor/snafu/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let locked_source =
            "git+https://github.com/shepmaster/snafu.git#1f8e75f56390c421a198871916100c6316d23d4f";
        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[("snafu", "0.9.0", locked_source)]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains(&format!("vendorSources.\"{locked_source}#snafu@0.9.0\"")));
        assert!(
            !rendered.contains(
                "vendorSources.\"git+https://github.com/shepmaster/snafu.git#snafu@0.9.0\""
            )
        );
    }

    #[test]
    fn git_vendor_sources_match_unit_graph_version_only_fragments() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "git+https://github.com/rust-netlink/rtnetlink?rev=eb685374ba7f7a1201754f6b2b40c491d3d50cb3#0.20.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "rtnetlink",
                    "src_path": "/vendor/rtnetlink/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let locked_source = "git+https://github.com/rust-netlink/rtnetlink?rev=eb685374ba7f7a1201754f6b2b40c491d3d50cb3#eb685374ba7f7a1201754f6b2b40c491d3d50cb3";
        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: Some(PathBuf::from("/vendor")),
                cargo_lock_sources: cargo_lock_sources(&[("rtnetlink", "0.20.0", locked_source)]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains(&format!(
            "vendorSources.\"{locked_source}#rtnetlink@0.20.0\""
        )));
    }

    #[cfg(unix)]
    #[test]
    fn builds_filtered_source_closure_when_package_symlinks_escape_root() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-symlink-source-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(workspace.join("internal")).unwrap();
        fs::create_dir_all(workspace.join("sibling/src")).unwrap();
        fs::write(
            workspace.join("internal/Cargo.toml"),
            r#"[package]
name = "internal"
version = "0.1.0"
"#,
        )
        .unwrap();
        fs::write(workspace.join("internal/lib.rs"), "pub fn marker() {}\n").unwrap();
        std::os::unix::fs::symlink("../sibling/src", workspace.join("internal/src")).unwrap();
        let src_path = workspace.join("internal/lib.rs");
        let pkg_id = format!(
            "path+file://{}#internal@0.1.0",
            workspace.join("internal").display()
        );
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "name": "internal",
                        "src_path": src_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                }
            ],
            "roots": [0]
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(
            rendered.contains("scopedWorkspaceClosureSource \"cargo-unit-source-internal-0.1.0-")
        );
        assert!(rendered.contains("[ \"internal\" \"sibling/src\" ]"));
        assert!(rendered.contains("export CARGO_MANIFEST_DIR=\"$src/internal\""));
        assert!(rendered.contains("\"$src/internal/lib.rs\""));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn extends_source_closure_through_include_macros() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-include-source-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(workspace.join("regex-lite/tests")).unwrap();
        fs::create_dir_all(workspace.join("testdata")).unwrap();
        fs::write(
            workspace.join("regex-lite/Cargo.toml"),
            r#"[package]
name = "regex-lite"
version = "0.1.0"
"#,
        )
        .unwrap();
        // The test entry point mirrors the regex-lite shape: an integration
        // test under `tests/` that reads sibling testdata via `include_bytes!`
        // with a parent-relative path. The walker must add the testdata dir
        // to the rustc source closure, otherwise the build sandbox can't see it.
        fs::write(
            workspace.join("regex-lite/tests/lib.rs"),
            r#"const ANCHORED: &[u8] = include_bytes!("../../testdata/anchored.toml");
const CRLF: &str = include_str!("../../testdata/crlf.toml");
"#,
        )
        .unwrap();
        fs::write(
            workspace.join("testdata/anchored.toml"),
            "name = 'anchored'\n",
        )
        .unwrap();
        fs::write(workspace.join("testdata/crlf.toml"), "name = 'crlf'\n").unwrap();
        let src_path = workspace.join("regex-lite/tests/lib.rs");
        let pkg_id = format!(
            "path+file://{}#regex-lite@0.1.0",
            workspace.join("regex-lite").display()
        );
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["test"],
                        "crate_types": ["bin"],
                        "name": "integration",
                        "src_path": src_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3", "test": true },
                    "mode": "build",
                    "dependencies": []
                }
            ],
            "roots": [0]
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("[ \"regex-lite\" \"testdata\" ]"));
        assert!(rendered.contains("sourcePackageRelative ="));
        assert!(rendered.contains("test_source_root="));
        assert!(rendered.contains("test_package_relative="));
        assert!(rendered.contains("test_cwd=\"$test_root/$test_package_relative\""));
        assert!(rendered.contains("cp -R \"$test_source_root\"/. \"$test_root\"/"));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn include_macro_does_not_promote_vendor_root_to_source_closure() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-vendor-include-boundary-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let vendor = workspace.join("vendor");
        let real = workspace.join("real");
        let clap = real.join("clap-4.6.1");
        let derive_arbitrary = real.join("derive_arbitrary-1.4.2");
        fs::create_dir_all(clap.join("src")).unwrap();
        fs::create_dir_all(derive_arbitrary.join("src")).unwrap();
        fs::create_dir_all(&vendor).unwrap();
        fs::write(
            clap.join("Cargo.toml"),
            r#"[package]
name = "clap"
version = "4.6.1"
"#,
        )
        .unwrap();
        fs::write(
            clap.join("src/lib.rs"),
            r#"#![doc = include_str!("../../README.md")]
"#,
        )
        .unwrap();
        fs::write(derive_arbitrary.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
        fs::write(vendor.join("README.md"), "vendor readme\n").unwrap();
        std::os::unix::fs::symlink(&clap, vendor.join("clap-4.6.1")).unwrap();
        std::os::unix::fs::symlink(&derive_arbitrary, vendor.join("derive_arbitrary-1.4.2"))
            .unwrap();
        let src_path = vendor.join("clap-4.6.1/src/lib.rs");
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#clap@4.6.1",
                    "target": {
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "name": "clap",
                        "src_path": src_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                }
            ],
            "roots": [0]
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: Some(vendor),
                cargo_lock_sources: cargo_lock_sources(&[(
                    "clap",
                    "4.6.1",
                    "registry+https://github.com/rust-lang/crates.io-index",
                )]),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("[ \"README.md\" \"clap-4.6.1\" ]"));
        assert!(!rendered.contains("derive_arbitrary-1.4.2"));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn extracts_literal_include_macro_paths_in_order() {
        let source = r#"
            const A: &[u8] = include_bytes!("data/anchored.toml");
            const B: &str = include_str!("../../shared/template.txt");
            // The filename stays dynamic, but the directory is static:
            include_bytes!(concat!("../../testdata/", CASE, ".toml"));
            // OUT_DIR paths are not source paths and are skipped:
            include!(concat!(env!("OUT_DIR"), "/generated.rs"));
            // Raw strings without `#` are still literals:
            const C: &[u8] = include_bytes!(r"raw/data.bin");
            // Wrong macro name with the same suffix; the word boundary blocks it:
            let _ = my_include_bytes!("not_a_real_macro");
            // Comments and intra-string occurrences are over-matched but harmless:
            // include_str!("from_a_comment.txt")
        "#;
        let mut paths = extract_include_macro_paths(source);
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "../../shared/template.txt".to_string(),
                "../../testdata".to_string(),
                "data/anchored.toml".to_string(),
                "from_a_comment.txt".to_string(),
                "raw/data.bin".to_string(),
            ]
        );
    }

    #[test]
    fn rejects_unscoped_local_sources() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///repo/crates/alpha#alpha@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "alpha",
                    "src_path": "/repo/crates/alpha/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let error = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("outside workspace root"));
    }

    #[test]
    fn rejects_external_sources_without_vendor_root() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "registry+https://github.com/rust-lang/crates.io-index#itoa@1.0.15",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "itoa",
                    "src_path": "/vendor/itoa-1.0.15/src/lib.rs",
                    "edition": "2021"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let error = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("needs --vendor-root"));
    }

    #[test]
    fn content_addressed_is_explicitly_opt_in() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "hello 0.1.0 (path+file:///workspace)",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "hello",
                    "src_path": "/workspace/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "dev", "opt_level": "0" },
                  "mode": "build",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: true,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("__contentAddressed = true"));
        assert!(rendered.contains("outputHashMode = \"recursive\""));
    }

    #[test]
    fn target_linker_environment_is_forwarded_to_rustc() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "platform": "x86_64-apple-darwin",
                  "dependencies": []
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(
            rendered
                .contains("if [ \"''${CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER+x}\" = x ]; then")
        );
        assert!(rendered.contains(
            "rustc_args+=( -C \"linker=''${CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER}\" )"
        ));
        assert!(rendered.contains("--target"));
        assert!(rendered.contains("x86_64-apple-darwin"));
    }

    #[test]
    fn extra_rustc_args_are_requested_for_the_unit_platform() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [
                {
                  "pkg_id": "path+file:///workspace#host@0.1.0",
                  "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "host",
                    "src_path": "/workspace/src/lib.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "dependencies": []
                },
                {
                  "pkg_id": "path+file:///workspace#hello@0.1.0",
                  "target": {
                    "kind": ["bin"],
                    "crate_types": ["bin"],
                    "name": "hello",
                    "src_path": "/workspace/src/main.rs",
                    "edition": "2024"
                  },
                  "profile": { "name": "release", "opt_level": "3" },
                  "mode": "build",
                  "platform": "x86_64-apple-darwin",
                  "dependencies": [
                    { "index": 0, "extern_crate_name": "host" }
                  ]
                }
              ],
              "roots": [1]
            }"#,
        )
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: PathBuf::from("/workspace"),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("extraRustcArgsForPlatform ? _platform: []"));
        assert!(rendered.contains("${renderExtraRustcArgs null}"));
        assert!(rendered.contains("${renderExtraRustcArgs \"x86_64-apple-darwin\"}"));
    }

    #[test]
    fn empty_shell_env_values_do_not_close_generated_nix_strings() {
        assert_eq!(shell_env_value(""), "\"\"");
        assert_eq!(
            shell_env_value("compiler's ${api}"),
            r#""compiler's \''${api}""#
        );
    }

    #[test]
    fn cargo_manifest_links_reads_dotted_package_keys() {
        let links = cargo_manifest_package_links(
            r#"
package.name = "native"
package.version = "0.1.0"
package.links = "native_ffi"
"#,
        )
        .unwrap();

        assert_eq!(links.as_deref(), Some("native_ffi"));
    }

    #[test]
    fn cargo_manifest_links_rejects_non_string_values() {
        let err = cargo_manifest_package_links(
            r#"
[package]
name = "native"
version = "0.1.0"
links = 5
"#,
        )
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("invalid type"),
            "expected TOML type error, got: {err}"
        );
    }

    #[test]
    fn build_script_runs_receive_cargo_target_cfg_and_feature_environment() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-render-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            r#"[package]
name = "native"
version = "0.1.0-alpha.1"
authors = ["Native Team", "Build Crew"]
description = "Native FFI fixtures"
homepage = "https://example.com/native"
repository = "https://example.com/native.git"
license = "MIT"
license-file = "LICENSE"
rust-version = "1.85"
links = "native_ffi"
"#,
        )
        .unwrap();
        let build_rs = workspace.join("build.rs");
        fs::write(&build_rs, "fn main() {}\n").unwrap();
        let build_rs_path = build_rs.to_string_lossy();
        let pkg_id = format!("path+file://{}#native@0.1.0-alpha.1", workspace.display());
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "features": ["arch", "simd-support"],
                    "mode": "build",
                    "dependencies": []
                },
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "features": ["arch", "simd-support"],
                    "mode": "run-custom-build",
                    "platform": "x86_64-unknown-linux-gnu",
                    "dependencies": [
                        { "index": 0, "extern_crate_name": "build_script_build" }
                    ]
                }
            ],
            "roots": []
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("export TARGET='x86_64-unknown-linux-gnu'"));
        assert!(rendered.contains("export CARGO_PKG_VERSION_PRE=\"alpha.1\""));
        assert!(rendered.contains("export CARGO_PKG_AUTHORS=\"Native Team:Build Crew\""));
        assert!(rendered.contains("export CARGO_PKG_DESCRIPTION=\"Native FFI fixtures\""));
        assert!(rendered.contains("export CARGO_PKG_HOMEPAGE=\"https://example.com/native\""));
        assert!(
            rendered.contains("export CARGO_PKG_REPOSITORY=\"https://example.com/native.git\"")
        );
        assert!(rendered.contains("export CARGO_PKG_LICENSE=\"MIT\""));
        assert!(rendered.contains("export CARGO_PKG_LICENSE_FILE=\"LICENSE\""));
        assert!(rendered.contains("export CARGO_PKG_RUST_VERSION=\"1.85\""));
        assert!(rendered.contains("export CARGO_MANIFEST_LINKS=\"native_ffi\""));
        assert!(rendered.contains("export CARGO_FEATURE_ARCH=1"));
        assert!(rendered.contains("export CARGO_FEATURE_SIMD_SUPPORT=1"));
        assert!(rendered.contains("\"$RUSTC\" --print cfg --target \"$TARGET\""));
        assert!(rendered.contains("cargo_cfg_env=\"CARGO_CFG_$(printf '%s' \"$cargo_cfg_key\""));
        assert!(
            rendered.contains("export \"$cargo_cfg_env=''${!cargo_cfg_env},$cargo_cfg_value\"")
        );
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn build_script_manifest_dir_uses_package_root_for_nested_entrypoints() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-nested-build-script-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        fs::create_dir_all(workspace.join("builder")).unwrap();
        fs::write(
            workspace.join("Cargo.toml"),
            r#"[package]
name = "nested-native"
version = "0.1.0"
links = "nested_native"
"#,
        )
        .unwrap();
        let build_rs = workspace.join("builder").join("main.rs");
        fs::write(&build_rs, "fn main() {}\n").unwrap();
        let build_rs_path = build_rs.to_string_lossy();
        let pkg_id = format!("path+file://{}#nested-native@0.1.0", workspace.display());
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-main",
                        "src_path": build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                },
                {
                    "pkg_id": pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-main",
                        "src_path": build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "run-custom-build",
                    "dependencies": [
                        { "index": 0, "extern_crate_name": "build_script_main" }
                    ]
                }
            ],
            "roots": []
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("export CARGO_MANIFEST_DIR=\"$src\""));
        assert!(!rendered.contains("export CARGO_MANIFEST_DIR=\"$src/builder\""));
        assert!(rendered.contains("export CARGO_MANIFEST_LINKS=\"nested_native\""));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn build_script_runs_receive_dependency_metadata_environment() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-dependency-metadata-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let sys_root = workspace.join("native-sys");
        let app_root = workspace.join("app");
        fs::create_dir_all(&sys_root).unwrap();
        fs::create_dir_all(&app_root).unwrap();
        fs::write(
            sys_root.join("Cargo.toml"),
            r#"[package]
name = "native-sys"
version = "0.1.0"
links = "native-ffi"
"#,
        )
        .unwrap();
        fs::write(
            app_root.join("Cargo.toml"),
            r#"[package]
name = "app"
version = "0.1.0"
"#,
        )
        .unwrap();
        let sys_build_rs = sys_root.join("build.rs");
        let app_build_rs = app_root.join("build.rs");
        fs::write(&sys_build_rs, "fn main() {}\n").unwrap();
        fs::write(&app_build_rs, "fn main() {}\n").unwrap();
        let sys_build_rs_path = sys_build_rs.to_string_lossy();
        let app_build_rs_path = app_build_rs.to_string_lossy();
        let sys_pkg_id = format!("path+file://{}#native-sys@0.1.0", sys_root.display());
        let app_pkg_id = format!("path+file://{}#app@0.1.0", app_root.display());
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": sys_pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": sys_build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                },
                {
                    "pkg_id": sys_pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": sys_build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "run-custom-build",
                    "dependencies": [
                        { "index": 0, "extern_crate_name": "build_script_build" }
                    ]
                },
                {
                    "pkg_id": app_pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": app_build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                },
                {
                    "pkg_id": app_pkg_id,
                    "target": {
                        "kind": ["custom-build"],
                        "crate_types": ["bin"],
                        "name": "build-script-build",
                        "src_path": app_build_rs_path,
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "run-custom-build",
                    "dependencies": [
                        { "index": 2, "extern_crate_name": "build_script_build" },
                        { "index": 1, "extern_crate_name": "native_sys" }
                    ]
                }
            ],
            "roots": []
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(
            rendered.contains(
                "cargo_metadata_env=\"DEP_NATIVE_FFI_$(printf '%s' \"$cargo_metadata_key\""
            )
        );
        assert!(rendered.contains("export \"$cargo_metadata_env=$cargo_metadata_value\""));
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn unit_graph_lint_flags_render_as_rustc_args() {
        let workspace = std::env::temp_dir().join(format!(
            "nix-cargo-unit-lint-flags-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        let _ = fs::remove_dir_all(&workspace);
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();

        let src_path = workspace.join("src/lib.rs");
        let pkg_id = format!("path+file://{}#linted@0.1.0", workspace.display());
        let graph: UnitGraph = serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [{
                "pkg_id": pkg_id,
                "target": {
                    "kind": ["lib"],
                    "crate_types": ["lib"],
                    "name": "linted",
                    "src_path": src_path,
                    "edition": "2024"
                },
                "profile": { "name": "dev", "opt_level": "0" },
                "lint_rustflags": [
                    "--deny=clippy::all",
                    "--forbid=unsafe_code",
                    "--warn=unexpected_cfgs",
                    "--warn=clippy::pedantic",
                    "--check-cfg",
                    "cfg(ix_test)"
                ],
                "check_cfg_args": [
                    "--check-cfg",
                    "cfg(docsrs,test)",
                    "--check-cfg",
                    "cfg(feature, values(\"alpha\", \"beta\"))"
                ],
                "mode": "build",
                "dependencies": []
            }],
            "roots": [0]
        }))
        .unwrap();

        let rendered = render_units_nix(
            &graph,
            &RenderOptions {
                workspace_root: workspace.clone(),
                vendor_root: None,
                cargo_lock_sources: CargoLockSources::default(),
                content_addressed: false,
                toolchain_id: None,
                deny_unused_crate_dependencies: false,
            },
        )
        .unwrap();

        assert!(rendered.contains("rustc_args+=( '--deny=clippy::all' )"));
        assert!(rendered.contains("rustc_args+=( '--forbid=unsafe_code' )"));
        assert!(rendered.contains("rustc_args+=( '--warn=clippy::pedantic' )"));
        assert!(rendered.contains("rustc_args+=( '--warn=unexpected_cfgs' )"));
        assert!(rendered.contains("rustc_args+=( '--check-cfg' )"));
        assert!(rendered.contains("rustc_args+=( 'cfg(docsrs,test)' )"));
        assert!(rendered.contains("rustc_args+=( 'cfg(feature, values(\"alpha\", \"beta\"))' )"));
        assert!(rendered.contains("rustc_args+=( 'cfg(ix_test)' )"));
        fs::remove_dir_all(workspace).unwrap();
    }
}
