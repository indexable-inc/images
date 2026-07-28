//! Relocation-based panic-reachability scan for compiled units.
//!
//! A function that can panic emits a call to a `core::panicking::*` entrypoint.
//! In a relocatable object (the `.o` members inside an rlib) that call survives
//! as a relocation whose target is the undefined panic symbol, located at an
//! offset inside the calling function's text range. Reading symbols and
//! relocations with the `object` crate attributes each panic call to its
//! containing function without disassembling instructions, so the same logic
//! covers ELF and Mach-O.
//!
//! This operates on relocatable objects (the `--emit obj` output of each unit),
//! not on linked binaries: a linked binary resolves its panic calls to direct
//! branches with no relocation left to read, which would need disassembly.
//! Generic functions are codegened where they are monomorphized, so a generic
//! that carries no relocation in its defining library's objects does carry one
//! in the bin object that instantiates it. Scanning every production unit's
//! objects and scoping findings to the workspace crate set therefore attributes
//! a monomorphized library generic back to its defining crate. Test and bench
//! units are not scanned: their bodies legitimately panic.
//!
//! This is a best-effort detector, not a soundness proof. A clean result means
//! "no detected panic call from workspace code reachable through the scanned
//! units," not "cannot panic." Two classes slip through by construction:
//!
//! - Generics no production unit instantiates. A public generic that no bin in
//!   the workspace ever monomorphizes is never codegened, so it carries no
//!   relocation anywhere here. It is also not reachable from the workspace's own
//!   production entrypoints, but an external downstream consumer could still hit
//!   a panic in it.
//! - Panics through uncatalogued helpers. [`PANIC_SINKS`] lists the common std
//!   sinks (`core::panicking`, `unwrap_failed`, `expect_failed`); a panic that
//!   routes through some other std/alloc cold path is missed until its symbol
//!   is added.
//!
//! Proving total panic-freedom needs call-graph reachability over the linked,
//! monomorphized binary (what `findpanics` does); that is the sound successor to
//! this per-unit check, not a tweak to it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, WrapErr as _};
use object::read::archive::ArchiveFile;
use object::{Object as _, ObjectSection as _, ObjectSymbol as _, RelocationTarget, SymbolSection};

/// One function that reaches panic machinery, with the entrypoint it calls.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PanicFinding {
    /// Mangled symbol of the function whose body holds the panic call.
    pub function: String,
    /// Mangled `core::panicking::*` symbol the function references.
    pub panic_entrypoint: String,
}

/// Scans every artifact path for functions that reach panic machinery.
///
/// When `crate_tokens` is non-empty, a finding is kept only if the function's
/// mangled symbol carries one of those crate tokens, or if it is unmangled (an
/// FFI export defined here). This is the set of workspace crate tokens, so a
/// library generic monomorphized inside a bin or test object is attributed to
/// its defining workspace crate and caught, while third-party and std
/// instantiations in the same object are ignored. An empty slice disables the
/// filter and reports every panic-reaching function.
pub fn scan_paths(paths: &[PathBuf], crate_tokens: &[String]) -> Result<Vec<PanicFinding>> {
    let mut findings = BTreeSet::new();
    for path in paths {
        let data = fs::read(path)
            .wrap_err_with(|| format!("reading artifact {} for panic scan", path.display()))?;
        scan_bytes(&data, crate_tokens, &mut findings)
            .wrap_err_with(|| format!("scanning artifact {} for panic calls", path.display()))?;
    }
    Ok(findings.into_iter().collect())
}

/// Length-prefixed crate token shared by legacy (`_ZN7n_hello`) and v0
/// (`_RNvCs..._7n_hello`) mangling. Cargo normalizes `-` to `_` in crate names.
pub fn crate_token(crate_name: &str) -> String {
    let normalized = crate_name.replace('-', "_");
    format!("{}{normalized}", normalized.len())
}

fn scan_bytes(
    data: &[u8],
    crate_tokens: &[String],
    findings: &mut BTreeSet<PanicFinding>,
) -> Result<()> {
    // An rlib is an `ar` archive of object members; a bare `.o` is parsed
    // directly. `ArchiveFile::parse` only succeeds on the archive magic, so a
    // failed parse means this is a single object, not a silent fallback.
    if let Ok(archive) = ArchiveFile::parse(data) {
        for member in archive.members() {
            let member = member.wrap_err("reading rlib archive member")?;
            let member_data = member.data(data).wrap_err("reading rlib member data")?;
            // Archives carry non-object members (rmeta, symbol index); those are
            // not objects and are skipped, but a member that claims to be an
            // object and fails to parse is a real error.
            if member.name().ends_with(b".o") {
                let object = object::File::parse(member_data)
                    .wrap_err("parsing rlib object member for panic scan")?;
                scan_object(&object, crate_tokens, findings);
            }
        }
        return Ok(());
    }
    // A collected artifact that is neither an archive nor a parseable object is
    // a corrupt or unsupported input. Fail closed: the panic gate must not pass
    // just because it could not read what it was handed.
    let object = object::File::parse(data)
        .wrap_err("artifact is neither an rlib archive nor a parseable object")?;
    scan_object(&object, crate_tokens, findings);
    Ok(())
}

struct FunctionRange {
    start: u64,
    end: u64,
    name: String,
}

fn scan_object(
    object: &object::File,
    crate_tokens: &[String],
    findings: &mut BTreeSet<PanicFinding>,
) {
    for section in object.sections() {
        let functions = function_ranges(object, section.index());
        for (offset, relocation) in section.relocations() {
            let RelocationTarget::Symbol(symbol_index) = relocation.target() else {
                continue;
            };
            let Ok(target) = object.symbol_by_index(symbol_index) else {
                continue;
            };
            let Ok(target_name) = target.name() else {
                continue;
            };
            if !is_panic_entrypoint(target_name) {
                continue;
            }
            if let Some(function) = containing_function(&functions, offset)
                && belongs_to_workspace(&function.name, crate_tokens)
            {
                findings.insert(PanicFinding {
                    function: function.name.clone(),
                    panic_entrypoint: target_name.to_string(),
                });
            }
        }
    }
}

// Text symbols defined in this section, sorted by address with each function's
// end clamped to the next function's start. Mach-O omits symbol sizes, so the
// neighbor's address is the only reliable upper bound.
fn function_ranges(object: &object::File, section: object::SectionIndex) -> Vec<FunctionRange> {
    let mut ranges: Vec<FunctionRange> = object
        .symbols()
        .filter(|symbol| symbol.section() == SymbolSection::Section(section))
        .filter(|symbol| symbol.kind() == object::SymbolKind::Text)
        .filter_map(|symbol| {
            let name = symbol.name().ok()?.to_string();
            let start = symbol.address();
            let end = start.checked_add(symbol.size()).filter(|end| *end > start);
            Some(FunctionRange {
                start,
                end: end.unwrap_or(u64::MAX),
                name,
            })
        })
        .collect();

    ranges.sort_by_key(|range| range.start);
    for index in 0..ranges.len() {
        if let Some(next_start) = ranges.get(index + 1).map(|next| next.start) {
            ranges[index].end = ranges[index].end.min(next_start);
        }
    }
    ranges
}

fn containing_function(functions: &[FunctionRange], offset: u64) -> Option<&FunctionRange> {
    functions
        .iter()
        .find(|function| offset >= function.start && offset < function.end)
}

// Curated set of std panic sinks, matched as length-prefixed path fragments
// rooted at the `core` / `std` crate so the same needles hit legacy
// (`_ZN4core9panicking...`) and v0 (`_RNvNt...4core9panicking...`) mangling.
//
// `core::panicking::*` is the leaf for `panic!`, formatting panics, bounds
// checks, overflow, and asserts. `unwrap`/`expect` reach a panic through the
// cold `unwrap_failed` / `expect_failed` helpers first (confirmed by inspecting
// the relocations rustc emits at opt-level 0 and 3), so those are listed too.
// This catalog is deliberately incomplete: see the false-negative note in the
// module docs.
const PANIC_SINKS: &[&str] = &[
    "4core9panicking",
    "3std9panicking",
    "4core6option13unwrap_failed",
    "4core6option13expect_failed",
    "4core6result13unwrap_failed",
    "4core6result13expect_failed",
];

fn is_panic_entrypoint(symbol: &str) -> bool {
    PANIC_SINKS
        .iter()
        .any(|sink| at_crate_boundary(symbol, sink))
}

// `core` / `std` only count as a panic sink when they are the crate root, not a
// nested user module. In v0 mangling the crate name follows the `Cs<id>_`
// disambiguator, so it is preceded by `_`; in legacy mangling the crate is the
// first path component after `_ZN`. A user path like `crate::core::panicking`
// puts `4core` after a crate-name character instead, so it never matches.
fn at_crate_boundary(symbol: &str, sink: &str) -> bool {
    symbol.match_indices(sink).any(|(index, _)| {
        let prefix = &symbol[..index];
        prefix.ends_with('_') || prefix.ends_with("ZN")
    })
}

// A function belongs to the workspace if its mangled symbol carries one of the
// workspace crate tokens at a crate-root boundary, or if it is not Rust-mangled
// at all: an `#[unsafe(no_mangle)]` export keeps a plain C name with no crate
// token, yet it is still defined in this crate's object, so excluding it would
// hide panics in FFI entrypoints. The boundary check (not a bare substring)
// keeps a short crate name like `de` from matching a dependency symbol such as
// `serde2de`. An empty token slice disables filtering.
fn belongs_to_workspace(function: &str, crate_tokens: &[String]) -> bool {
    crate_tokens.is_empty()
        || !is_rust_mangled(function)
        || crate_tokens
            .iter()
            .any(|token| at_crate_boundary(function, token))
}

fn is_rust_mangled(symbol: &str) -> bool {
    let stripped = symbol
        .strip_prefix("__")
        .or_else(|| symbol.strip_prefix('_'))
        .unwrap_or(symbol);
    stripped.starts_with("ZN") || stripped.starts_with('R')
}

/// Collects scannable artifacts (`*.rlib` archives and `*.o` objects) under each
/// input path. A path that is itself a file is taken as-is so callers can pass
/// exact artifacts.
pub fn collect_artifacts(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut artifacts = Vec::new();
    for root in roots {
        collect_artifacts_into(root, &mut artifacts)?;
    }
    artifacts.sort();
    Ok(artifacts)
}

fn collect_artifacts_into(root: &Path, artifacts: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .wrap_err_with(|| format!("inspecting panic-scan path {}", root.display()))?;
    if metadata.is_file() {
        artifacts.push(root.to_path_buf());
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(root).wrap_err_with(|| format!("reading directory {}", root.display()))?
        {
            let entry =
                entry.wrap_err_with(|| format!("reading entry under {}", root.display()))?;
            let path = entry.path();
            if path.is_dir()
                || path
                    .extension()
                    .is_some_and(|ext| ext == "rlib" || ext == "o")
            {
                collect_artifacts_into(&path, artifacts)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write::{
        Object, Relocation, RelocationFlags, StandardSection, Symbol, SymbolSection as WriteSection,
    };
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };

    // Builds a relocatable ELF object with one 16-byte text function. When
    // `callee` is set, a relocation at offset 4 targets that undefined symbol,
    // modelling a call from the function into the named callee.
    fn object_calling(function: &str, callee: Option<&str>) -> Vec<u8> {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0u8; 16], 1);
        object.add_symbol(Symbol {
            name: function.as_bytes().to_vec(),
            value: 0,
            size: 16,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: WriteSection::Section(text),
            flags: SymbolFlags::None,
        });
        if let Some(callee) = callee {
            let target = object.add_symbol(Symbol {
                name: callee.as_bytes().to_vec(),
                value: 0,
                size: 0,
                kind: SymbolKind::Text,
                scope: SymbolScope::Dynamic,
                weak: false,
                section: WriteSection::Undefined,
                flags: SymbolFlags::None,
            });
            object
                .add_relocation(
                    text,
                    Relocation {
                        offset: 4,
                        symbol: target,
                        addend: 0,
                        flags: RelocationFlags::Generic {
                            kind: RelocationKind::PltRelative,
                            encoding: RelocationEncoding::X86Branch,
                            size: 32,
                        },
                    },
                )
                .expect("add relocation");
        }
        object.write().expect("serialize fixture object")
    }

    fn object_with_function(function: &str, panic: bool) -> Vec<u8> {
        let callee = panic.then_some("_ZN4core9panicking18panic_bounds_check17hababababababababE");
        object_calling(function, callee)
    }

    // Scans with the given crate names as the workspace filter (empty = no
    // filter), mirroring how the renderer passes the workspace crate set.
    fn scan(data: &[u8], crate_names: &[&str]) -> Vec<PanicFinding> {
        let tokens: Vec<String> = crate_names.iter().map(|name| crate_token(name)).collect();
        let mut findings = BTreeSet::new();
        scan_bytes(data, &tokens, &mut findings).expect("scan fixture");
        findings.into_iter().collect()
    }

    #[test]
    fn flags_function_that_calls_panic_entrypoint() {
        let bytes = object_with_function("_ZN7n_hello3get17habcdefgEhh", true);
        let findings = scan(&bytes, &[]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].function, "_ZN7n_hello3get17habcdefgEhh");
        assert!(findings[0].panic_entrypoint.contains("panic_bounds_check"));
    }

    #[test]
    fn clean_function_produces_no_findings() {
        let bytes = object_with_function("_ZN7n_hello5clean17habcdefgEhh", false);
        assert!(scan(&bytes, &[]).is_empty());
    }

    #[test]
    fn workspace_filter_excludes_foreign_functions() {
        // A panic call lives in a `serde`-named function. A workspace set that
        // does not include serde must drop it; one that does keeps it.
        let bytes = object_with_function("_ZN5serde2de17habcdefgEhh", true);
        assert!(scan(&bytes, &["n-hello"]).is_empty());
        assert_eq!(scan(&bytes, &["serde"]).len(), 1);
    }

    #[test]
    fn workspace_set_catches_library_generic_in_consumer_object() {
        // A library generic monomorphized in a consumer object keeps the
        // library's crate token. Scanning the consumer with only the consumer
        // crate misses it; the full workspace set catches it.
        let bytes = object_calling(
            "_ZN6thelib5first17habcdefgEhh",
            Some("_ZN4core9panicking18panic_bounds_check17habcdefgEhh"),
        );
        assert!(scan(&bytes, &["app"]).is_empty());
        assert_eq!(scan(&bytes, &["app", "thelib"]).len(), 1);
    }

    #[test]
    fn crate_token_normalizes_dashes() {
        assert_eq!(crate_token("n-hello"), "7n_hello");
        assert_eq!(crate_token("serde"), "5serde");
    }

    #[test]
    fn flags_unwrap_and_expect_cold_paths() {
        // unwrap/expect reach a panic through the *_failed helpers, not
        // core::panicking directly, so the catalog must catch them.
        for callee in [
            "_ZN4core6option13unwrap_failed17habcdefgEhh",
            "_ZN4core6result13unwrap_failed17habcdefgEhh",
            "_ZN4core6option13expect_failed17habcdefgEhh",
        ] {
            let bytes = object_calling("_ZN7n_hello3run17habcdefgEhh", Some(callee));
            assert_eq!(scan(&bytes, &[]).len(), 1, "missed cold path {callee}");
        }
    }

    #[test]
    fn user_panicking_module_is_not_a_panic_sink() {
        // A crate's own `panicking` module mangles with its crate prefix, not
        // `4core` / `3std`, so calling it must not trip the gate.
        let bytes = object_calling(
            "_ZN7n_hello3run17habcdefgEhh",
            Some("_ZN7n_hello9panicking6record17habcdefgEhh"),
        );
        assert!(scan(&bytes, &[]).is_empty());
    }

    #[test]
    fn nested_user_core_module_is_not_a_panic_sink() {
        // `crate::core::panicking::helper` mangles with the user crate as root,
        // so `4core` sits after a crate-name character, not the crate boundary.
        let bytes = object_calling(
            "_ZN7n_hello3run17habcdefgEhh",
            Some("_ZN7n_hello4core9panicking6helper17habcdefgEhh"),
        );
        assert!(scan(&bytes, &[]).is_empty());
    }

    #[test]
    fn unparseable_artifact_is_an_error() {
        // Fail closed: a corrupt or unsupported artifact must not scan as clean.
        let mut findings = BTreeSet::new();
        assert!(scan_bytes(b"not an object or archive", &[], &mut findings).is_err());
    }

    #[test]
    fn short_crate_token_does_not_match_dependency_substring() {
        // crate `de` (token `2de`) must not match serde's `de` module symbol,
        // which only contains the token mid-path, not at the crate boundary.
        let bytes = object_calling(
            "_ZN5serde2de9from_slice17habcdefgEhh",
            Some("_ZN4core9panicking5panic17habcdefgEhh"),
        );
        assert!(scan(&bytes, &["de"]).is_empty());
    }

    #[test]
    fn unmangled_export_is_scanned_under_workspace_filter() {
        // An `#[unsafe(no_mangle)]` export carries no crate token; a workspace
        // filter must still scan it, since it is defined in this crate's object.
        let bytes = object_calling("ffi_entry", Some("_ZN4core9panicking5panic17habcdefgEhh"));
        assert_eq!(scan(&bytes, &["n-hello"]).len(), 1);
    }
}
