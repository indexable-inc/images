use std::borrow::Cow;

use std::collections::BTreeMap;

use color_eyre::eyre::{Result as EyreResult, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use url::Url;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UnitGraph {
    pub version: u32,
    pub units: Vec<Unit>,
    pub roots: Vec<usize>,
    #[serde(default)]
    pub root_sets: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Unit {
    pub pkg_id: String,
    pub target: Target,
    pub profile: Profile,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub lint_rustflags: Vec<String>,
    #[serde(default)]
    pub check_cfg_args: Vec<String>,
    pub mode: UnitMode,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub is_std: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Target {
    #[serde(default)]
    pub kind: Vec<String>,
    #[serde(default)]
    pub crate_types: Vec<String>,
    pub name: String,
    pub src_path: String,
    pub edition: String,
    #[serde(default = "default_true")]
    pub test: bool,
    #[serde(default = "default_true")]
    pub doctest: bool,
    #[serde(default = "default_true")]
    pub doc: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Profile {
    pub name: String,
    pub opt_level: String,
    #[serde(default)]
    pub lto: Lto,
    #[serde(default)]
    pub codegen_units: Option<u32>,
    #[serde(default)]
    pub debuginfo: DebugInfo,
    #[serde(default)]
    pub debug_assertions: bool,
    #[serde(default)]
    pub overflow_checks: bool,
    #[serde(default)]
    pub rpath: bool,
    #[serde(default)]
    pub incremental: bool,
    #[serde(default)]
    pub panic: Panic,
    #[serde(default)]
    pub strip: Strip,
    #[serde(default)]
    pub split_debuginfo: Option<String>,
    #[serde(default)]
    pub rustflags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dependency {
    pub index: usize,
    pub extern_crate_name: String,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub noprelude: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UnitMode {
    Build,
    Check,
    Test,
    Doc,
    RunCustomBuild,
    Other(String),
}

impl UnitMode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Build => "build",
            Self::Check => "check",
            Self::Test => "test",
            Self::Doc => "doc",
            Self::RunCustomBuild => "run-custom-build",
            Self::Other(mode) => mode,
        }
    }
}

impl<'de> Deserialize<'de> for UnitMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mode = String::deserialize(deserializer)?;
        Ok(match mode.as_str() {
            "build" => Self::Build,
            "check" => Self::Check,
            "test" => Self::Test,
            "doc" => Self::Doc,
            "run-custom-build" => Self::RunCustomBuild,
            _ => Self::Other(mode),
        })
    }
}

impl Serialize for UnitMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum Lto {
    #[default]
    Off,
    Thin,
    Fat,
}

impl<'de> Deserialize<'de> for Lto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            serde_json::Value::Bool(true) => Self::Fat,
            serde_json::Value::String(value) => match value.as_str() {
                "true" | "fat" => Self::Fat,
                "thin" => Self::Thin,
                _ => Self::Off,
            },
            _ => Self::Off,
        })
    }
}

impl Serialize for Lto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Off => "off",
            Self::Thin => "thin",
            Self::Fat => "fat",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum DebugInfo {
    #[default]
    None,
    LineDirectivesOnly,
    LineTablesOnly,
    Limited,
    Full,
}

impl<'de> Deserialize<'de> for DebugInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            serde_json::Value::Bool(true) => Self::Full,
            serde_json::Value::Number(number) => match number.as_u64() {
                Some(0) => Self::None,
                Some(1) => Self::Limited,
                _ => Self::Full,
            },
            serde_json::Value::String(value) => match value.as_str() {
                "line-directives-only" => Self::LineDirectivesOnly,
                "line-tables-only" => Self::LineTablesOnly,
                "1" | "limited" => Self::Limited,
                "2" | "full" | "true" => Self::Full,
                _ => Self::None,
            },
            _ => Self::None,
        })
    }
}

impl Serialize for DebugInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "none",
            Self::LineDirectivesOnly => "line-directives-only",
            Self::LineTablesOnly => "line-tables-only",
            Self::Limited => "limited",
            Self::Full => "full",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Panic {
    #[default]
    Unwind,
    Abort,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub enum Strip {
    #[default]
    None,
    Debuginfo,
    Symbols,
}

impl<'de> Deserialize<'de> for Strip {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            serde_json::Value::Bool(true) => Self::Symbols,
            serde_json::Value::String(value) => strip_from_str(&value),
            serde_json::Value::Object(mut value) => value
                .remove("resolved")
                .and_then(|resolved| match resolved {
                    serde_json::Value::String(value) => Some(strip_from_str(&value)),
                    serde_json::Value::Object(mut inner) => inner
                        .remove("Named")
                        .and_then(|named| named.as_str().map(strip_from_str)),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => Self::None,
        })
    }
}

impl Serialize for Strip {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::None => "none",
            Self::Debuginfo => "debuginfo",
            Self::Symbols => "symbols",
        })
    }
}

fn strip_from_str(value: &str) -> Strip {
    match value {
        "debuginfo" => Strip::Debuginfo,
        "symbols" | "true" => Strip::Symbols,
        _ => Strip::None,
    }
}

const fn default_true() -> bool {
    true
}

impl UnitGraph {
    pub fn merge(graphs: Vec<Self>) -> EyreResult<Self> {
        let mut merged = Self {
            version: 1,
            units: Vec::new(),
            roots: Vec::new(),
            root_sets: Vec::new(),
        };
        let mut merged_by_hash = BTreeMap::new();

        for graph in graphs {
            graph.ensure_supported()?;
            let mut hashes = vec![None; graph.units.len()];
            for index in 0..graph.units.len() {
                merge_identity_hash(&graph, index, &mut hashes);
            }

            let mut index_map = vec![None; graph.units.len()];
            for index in 0..graph.units.len() {
                merge_unit(
                    &graph,
                    &hashes,
                    index,
                    &mut index_map,
                    &mut merged,
                    &mut merged_by_hash,
                );
            }

            let mut root_set = Vec::new();
            for root in graph.roots {
                let merged_root = index_map[root].expect("root unit was merged");
                if !merged.roots.contains(&merged_root) {
                    merged.roots.push(merged_root);
                }
                if !root_set.contains(&merged_root) {
                    root_set.push(merged_root);
                }
            }
            merged.root_sets.push(root_set);
        }

        merged.validate()?;
        Ok(merged)
    }

    pub fn ensure_supported(&self) -> EyreResult<()> {
        ensure!(
            self.version == 1,
            "unsupported cargo unit graph version {}",
            self.version
        );
        self.validate()
    }

    pub fn validate(&self) -> EyreResult<()> {
        for root in self.roots.iter().chain(self.root_sets.iter().flatten()) {
            if *root >= self.units.len() {
                bail!(
                    "root unit index {root} is outside the unit graph with {} units",
                    self.units.len()
                );
            }
        }

        for (unit_index, unit) in self.units.iter().enumerate() {
            for dependency in &unit.dependencies {
                if dependency.index >= self.units.len() {
                    bail!(
                        "unit {unit_index} dependency {} points to missing unit {} in graph with {} units",
                        dependency.extern_crate_name,
                        dependency.index,
                        self.units.len()
                    );
                }
            }
        }

        Ok(())
    }
}

// Infallible: `merge` validates every graph up front, so all indexes are in
// bounds.
fn merge_unit(
    graph: &UnitGraph,
    hashes: &[Option<String>],
    index: usize,
    index_map: &mut [Option<usize>],
    merged: &mut UnitGraph,
    merged_by_hash: &mut BTreeMap<String, usize>,
) -> usize {
    if let Some(merged_index) = index_map[index] {
        return merged_index;
    }

    let hash = hashes[index].clone().expect("unit hash was computed");
    let merged_index = if let Some(merged_index) = merged_by_hash.get(&hash) {
        *merged_index
    } else {
        let mut unit = graph.units[index].clone();
        unit.dependencies = unit
            .dependencies
            .iter()
            .map(|dependency| Dependency {
                index: merge_unit(
                    graph,
                    hashes,
                    dependency.index,
                    index_map,
                    merged,
                    merged_by_hash,
                ),
                extern_crate_name: dependency.extern_crate_name.clone(),
                public: dependency.public,
                noprelude: dependency.noprelude,
            })
            .collect();
        let merged_index = merged.units.len();
        merged.units.push(unit);
        merged_by_hash.insert(hash, merged_index);
        merged_index
    };

    index_map[index] = Some(merged_index);
    merged_index
}

fn merge_identity_hash(graph: &UnitGraph, index: usize, hashes: &mut [Option<String>]) -> String {
    if let Some(hash) = &hashes[index] {
        return hash.clone();
    }

    let unit = &graph.units[index];
    let dependency_hashes: Vec<String> = unit
        .dependencies
        .iter()
        .map(|dependency| {
            format!(
                "{}:{}:{}:{}",
                dependency.extern_crate_name,
                dependency.public,
                dependency.noprelude,
                merge_identity_hash(graph, dependency.index, hashes)
            )
        })
        .collect();
    let hash = unit.identity_hash(&dependency_hashes, None);
    hashes[index] = Some(hash.clone());
    hash
}

#[derive(Debug, Clone)]
pub struct PackageId<'a> {
    pub name: Cow<'a, str>,
    pub version: &'a str,
}

pub fn parse_pkg_id(pkg_id: &str) -> Option<PackageId<'_>> {
    if pkg_id.starts_with("path+")
        || pkg_id.starts_with("registry+")
        || pkg_id.starts_with("git+")
        || pkg_id.starts_with("sparse+")
    {
        let (scheme_loc, fragment) = pkg_id.rsplit_once('#')?;
        if let Some((name, version)) = fragment.split_once('@') {
            return Some(PackageId {
                name: Cow::Borrowed(name),
                version,
            });
        }

        let name = if let Some(location) = scheme_loc.strip_prefix("path+file://") {
            package_name_from_path_location(location)?
        } else {
            let location = scheme_loc
                .strip_prefix("git+")
                .or_else(|| scheme_loc.strip_prefix("registry+"))
                .or_else(|| scheme_loc.strip_prefix("sparse+"))?;
            package_name_from_url_location(location)?
        };

        return Some(PackageId {
            name,
            version: fragment,
        });
    }

    let mut parts = pkg_id.split_whitespace();
    Some(PackageId {
        name: Cow::Borrowed(parts.next()?),
        version: parts.next()?,
    })
}

fn package_name_from_path_location(location: &str) -> Option<Cow<'_, str>> {
    let name = location
        .rsplit('/')
        .next()
        .and_then(|name| name.strip_suffix(".git").or(Some(name)))?;
    Some(Cow::Borrowed(name))
}

fn package_name_from_url_location(location: &str) -> Option<Cow<'_, str>> {
    let url = Url::parse(location).ok()?;
    let segment = url.path_segments()?.rfind(|segment| !segment.is_empty())?;
    let name = segment.strip_suffix(".git").unwrap_or(segment);
    Some(Cow::Owned(name.to_string()))
}

impl Target {
    pub fn has_kind(&self, kind: &str) -> bool {
        self.kind.iter().any(|candidate| candidate == kind)
    }

    pub fn has_crate_type(&self, crate_type: &str) -> bool {
        self.crate_types
            .iter()
            .any(|candidate| candidate == crate_type)
    }

    pub fn has_library_kind(&self) -> bool {
        self.kind.iter().any(|kind| {
            matches!(
                kind.as_str(),
                "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
            )
        })
    }
}

impl Unit {
    pub fn package_name(&self) -> Cow<'_, str> {
        parse_pkg_id(&self.pkg_id).map_or_else(
            || Cow::Borrowed(self.target.name.as_str()),
            |package| package.name,
        )
    }

    pub fn package_version(&self) -> &str {
        parse_pkg_id(&self.pkg_id).map_or("0.0.0", |package| package.version)
    }

    pub fn is_bin(&self) -> bool {
        self.target.has_crate_type("bin") || self.target.has_kind("bin")
    }

    pub fn is_proc_macro(&self) -> bool {
        self.target.has_crate_type("proc-macro") || self.target.has_kind("proc-macro")
    }

    pub fn is_library(&self) -> bool {
        self.target.has_library_kind()
    }

    pub fn has_doctests(&self) -> bool {
        self.target.doctest
            && self.is_library()
            && !self.is_external()
            && self.mode != UnitMode::Test
    }

    pub fn is_custom_build_compile(&self) -> bool {
        self.target.has_kind("custom-build") && !self.is_run_custom_build()
    }

    pub fn is_run_custom_build(&self) -> bool {
        self.mode == UnitMode::RunCustomBuild
    }

    pub fn is_test(&self) -> bool {
        !self.is_benchmark() && (self.mode == UnitMode::Test || self.target.has_kind("test"))
    }

    pub fn is_benchmark(&self) -> bool {
        self.target.has_kind("bench")
    }

    pub fn uses_test_harness(&self) -> bool {
        self.target.test && (self.mode == UnitMode::Test || self.target.has_kind("test"))
    }

    pub fn is_external(&self) -> bool {
        self.pkg_id.starts_with("registry+")
            || self.pkg_id.starts_with("git+")
            || self.pkg_id.starts_with("sparse+")
            || self.pkg_id.contains("(registry+")
            || self.pkg_id.contains("(git+")
            || self.pkg_id.contains("(sparse+")
    }

    pub fn identity_hash(
        &self,
        dependency_hashes: &[String],
        toolchain_id: Option<&str>,
    ) -> String {
        let mut hasher = sha2::Sha256::new();
        write_unit_identity(&mut hasher, self);

        let mut dependency_hashes = dependency_hashes.to_vec();
        dependency_hashes.sort();
        for hash in dependency_hashes {
            hasher.update(b"dep\0");
            hasher.update(hash.as_bytes());
            hasher.update(b"\0");
        }

        if let Some(toolchain_id) = toolchain_id {
            hasher.update(b"toolchain\0");
            hasher.update(toolchain_id.as_bytes());
            hasher.update(b"\0");
        }

        crate::hash::short_digest(&hasher.finalize())
    }
}

fn write_unit_identity(hasher: &mut sha2::Sha256, unit: &Unit) {
    let package_identity = package_identity(unit);
    hasher.update(package_identity.as_bytes());
    hasher.update(b"\0");
    hasher.update(unit.target.name.as_bytes());
    hasher.update(b"\0");
    hasher.update(unit.target.edition.as_bytes());
    hasher.update(b"\0");

    let mut crate_types = unit.target.crate_types.clone();
    crate_types.sort();
    for crate_type in crate_types {
        hasher.update(crate_type.as_bytes());
        hasher.update(b"\0");
    }

    let mut features = unit.features.clone();
    features.sort();
    for feature in features {
        hasher.update(feature.as_bytes());
        hasher.update(b"\0");
    }

    hasher.update(unit.profile.name.as_bytes());
    hasher.update(b"\0");
    hasher.update(unit.profile.opt_level.as_bytes());
    hasher.update(b"\0");
    hasher.update([unit.profile.lto.identity_byte()]);
    hasher.update([unit.profile.debuginfo.identity_byte()]);
    hasher.update([unit.profile.panic.identity_byte()]);
    hasher.update([unit.profile.strip.identity_byte()]);
    hash_bool(hasher, unit.profile.debug_assertions);
    hash_bool(hasher, unit.profile.overflow_checks);
    hash_bool(hasher, unit.profile.rpath);
    hash_bool(hasher, unit.profile.incremental);
    if let Some(codegen_units) = unit.profile.codegen_units {
        hasher.update(codegen_units.to_string().as_bytes());
    }
    hasher.update(b"\0");
    if let Some(split_debuginfo) = &unit.profile.split_debuginfo {
        hasher.update(split_debuginfo.as_bytes());
    }
    hasher.update(b"\0");
    for flag in &unit.profile.rustflags {
        hasher.update(flag.as_bytes());
        hasher.update(b"\0");
    }
    for flag in &unit.lint_rustflags {
        hasher.update(flag.as_bytes());
        hasher.update(b"\0");
    }
    for arg in &unit.check_cfg_args {
        hasher.update(arg.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(unit.mode.as_str().as_bytes());
    hasher.update(b"\0");
    if let Some(platform) = &unit.platform {
        hasher.update(platform.as_bytes());
    }
    hasher.update(b"\0");
    hash_bool(hasher, unit.is_std);
    hash_bool(hasher, unit.target.test);
    hash_bool(hasher, unit.target.doctest);
    hash_bool(hasher, unit.target.doc);
}

fn package_identity(unit: &Unit) -> String {
    if unit.pkg_id.starts_with("path+") || unit.pkg_id.contains("(path+") {
        format!("path#{}@{}", unit.package_name(), unit.package_version())
    } else {
        unit.pkg_id.clone()
    }
}

fn hash_bool(hasher: &mut sha2::Sha256, value: bool) {
    hasher.update(if value { b"1" } else { b"0" });
    hasher.update(b"\0");
}

impl Lto {
    pub const fn identity_byte(self) -> u8 {
        match self {
            Self::Off => b'0',
            Self::Thin => b'1',
            Self::Fat => b'2',
        }
    }

    pub const fn rustc_value(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Thin => Some("thin"),
            Self::Fat => Some("fat"),
        }
    }
}

impl DebugInfo {
    pub const fn identity_byte(self) -> u8 {
        match self {
            Self::None => b'0',
            Self::LineDirectivesOnly => b'1',
            Self::LineTablesOnly => b'2',
            Self::Limited => b'3',
            Self::Full => b'4',
        }
    }

    pub const fn rustc_value(self) -> &'static str {
        match self {
            Self::None => "0",
            Self::LineDirectivesOnly => "line-directives-only",
            Self::LineTablesOnly => "line-tables-only",
            Self::Limited => "1",
            Self::Full => "2",
        }
    }

    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl Panic {
    pub const fn identity_byte(self) -> u8 {
        match self {
            Self::Unwind => b'0',
            Self::Abort => b'1',
        }
    }

    pub const fn rustc_value(self) -> &'static str {
        match self {
            Self::Unwind => "unwind",
            Self::Abort => "abort",
        }
    }
}

impl Strip {
    pub const fn identity_byte(self) -> u8 {
        match self {
            Self::None => b'0',
            Self::Debuginfo => b'1',
            Self::Symbols => b'2',
        }
    }

    pub const fn rustc_value(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Debuginfo => Some("debuginfo"),
            Self::Symbols => Some("symbols"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_mode_preserves_unknown_cargo_values() {
        let mode: UnitMode = serde_json::from_str(r#""future-mode""#).unwrap();

        assert_eq!(mode.as_str(), "future-mode");
    }

    #[test]
    fn validation_rejects_missing_root_units() {
        let graph: UnitGraph = serde_json::from_str(
            r#"{
              "version": 1,
              "units": [],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let error = graph.ensure_supported().unwrap_err().to_string();

        assert!(error.contains("root unit index 0"));
    }

    #[test]
    fn validation_rejects_missing_dependency_units() {
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
                  "profile": { "name": "dev", "opt_level": "0" },
                  "mode": "build",
                  "dependencies": [
                    { "index": 1, "extern_crate_name": "missing" }
                  ]
                }
              ],
              "roots": [0]
            }"#,
        )
        .unwrap();

        let error = graph.ensure_supported().unwrap_err().to_string();

        assert!(error.contains("unit 0 dependency missing"));
    }

    #[test]
    fn sparse_package_ids_parse_as_registry_packages() {
        let package = parse_pkg_id("sparse+https://index.crates.io/#serde@1.0.228").unwrap();

        assert_eq!(package.name, "serde");
        assert_eq!(package.version, "1.0.228");
    }

    #[test]
    fn git_package_ids_with_version_fragments_ignore_query_params_in_inferred_names() {
        let package = parse_pkg_id(
            "git+https://github.com/rust-netlink/rtnetlink?rev=eb685374ba7f7a1201754f6b2b40c491d3d50cb3#0.20.0",
        )
        .unwrap();

        assert_eq!(package.name, "rtnetlink");
        assert_eq!(package.version, "0.20.0");
    }

    #[test]
    fn path_package_identity_ignores_absolute_source_roots() {
        let left: Unit = serde_json::from_str(
            r#"{
              "pkg_id": "path+file:///nix/store/source-left/crates/alpha#alpha@0.1.0",
              "target": {
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "alpha",
                "src_path": "/nix/store/source-left/crates/alpha/src/lib.rs",
                "edition": "2024"
              },
              "profile": { "name": "release", "opt_level": "3" },
              "mode": "build",
              "dependencies": []
            }"#,
        )
        .unwrap();
        let right: Unit = serde_json::from_str(
            r#"{
              "pkg_id": "path+file:///nix/store/source-right/crates/alpha#alpha@0.1.0",
              "target": {
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "alpha",
                "src_path": "/nix/store/source-right/crates/alpha/src/lib.rs",
                "edition": "2024"
              },
              "profile": { "name": "release", "opt_level": "3" },
              "mode": "build",
              "dependencies": []
            }"#,
        )
        .unwrap();

        assert_eq!(
            left.identity_hash(&[], None),
            right.identity_hash(&[], None)
        );
    }

    fn graph_with_dependency_edge(
        extern_crate_name: &str,
        public: bool,
        noprelude: bool,
    ) -> UnitGraph {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "units": [
                {
                    "pkg_id": "path+file:///workspace/dep#dep@0.1.0",
                    "target": {
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "name": "dep",
                        "src_path": "/workspace/dep/src/lib.rs",
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": []
                },
                {
                    "pkg_id": "path+file:///workspace/app#app@0.1.0",
                    "target": {
                        "kind": ["lib"],
                        "crate_types": ["lib"],
                        "name": "app",
                        "src_path": "/workspace/app/src/lib.rs",
                        "edition": "2024"
                    },
                    "profile": { "name": "release", "opt_level": "3" },
                    "mode": "build",
                    "dependencies": [
                        {
                            "index": 0,
                            "extern_crate_name": extern_crate_name,
                            "public": public,
                            "noprelude": noprelude
                        }
                    ]
                }
            ],
            "roots": [1]
        }))
        .unwrap()
    }

    #[test]
    fn merge_keeps_units_with_different_dependency_edges() {
        let merged = UnitGraph::merge(vec![
            graph_with_dependency_edge("dep", false, false),
            graph_with_dependency_edge("renamed_dep", true, true),
        ])
        .unwrap();

        assert_eq!(merged.units.len(), 3);
        assert_eq!(merged.roots.len(), 2);
        assert_ne!(merged.roots[0], merged.roots[1]);
        assert_eq!(
            merged.units[merged.roots[0]].dependencies[0].extern_crate_name,
            "dep"
        );
        assert_eq!(
            merged.units[merged.roots[1]].dependencies[0].extern_crate_name,
            "renamed_dep"
        );
        assert!(merged.units[merged.roots[1]].dependencies[0].public);
        assert!(merged.units[merged.roots[1]].dependencies[0].noprelude);
    }
}
