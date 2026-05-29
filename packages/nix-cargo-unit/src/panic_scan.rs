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
//! This operates on relocatable objects, which is why it targets library rlibs:
//! a fully linked binary has its panic calls resolved to direct branches with
//! no relocation left to read, and recovering them needs disassembly. Scanning
//! linked bin/test units is deliberately out of scope here.

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
/// When `crate_token` is `Some`, only functions whose mangled symbol carries
/// that crate's length-prefixed name component are reported, so the gate covers
/// the unit's own code rather than monomorphized helpers from its dependencies.
pub fn scan_paths(paths: &[PathBuf], crate_token: Option<&str>) -> Result<Vec<PanicFinding>> {
    let mut findings = BTreeSet::new();
    for path in paths {
        let data = fs::read(path)
            .wrap_err_with(|| format!("reading artifact {} for panic scan", path.display()))?;
        scan_bytes(&data, crate_token, &mut findings)
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
    crate_token: Option<&str>,
    findings: &mut BTreeSet<PanicFinding>,
) -> Result<()> {
    // An rlib is an `ar` archive of object members; a bare `.o` is parsed
    // directly. `ArchiveFile::parse` only succeeds on the archive magic, so a
    // failed parse means this is a single object, not a silent fallback.
    if let Ok(archive) = ArchiveFile::parse(data) {
        for member in archive.members() {
            let member = member.wrap_err("reading rlib archive member")?;
            let member_data = member.data(data).wrap_err("reading rlib member data")?;
            if let Ok(object) = object::File::parse(member_data) {
                scan_object(&object, crate_token, findings);
            }
        }
    } else if let Ok(object) = object::File::parse(data) {
        scan_object(&object, crate_token, findings);
    }
    Ok(())
}

struct FunctionRange {
    start: u64,
    end: u64,
    name: String,
}

fn scan_object(
    object: &object::File,
    crate_token: Option<&str>,
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
                && crate_token.is_none_or(|token| function.name.contains(token))
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

// Both legacy and v0 mangling encode the path component `core::panicking` as the
// length-prefixed run `9panicking`, so the substring identifies every panic
// entrypoint (`panic`, `panic_fmt`, `panic_bounds_check`, ...) regardless of
// mangling scheme or the Mach-O leading-underscore prefix.
fn is_panic_entrypoint(symbol: &str) -> bool {
    symbol.contains("9panicking")
}

/// Collects `*.rlib` artifacts under each input path. A path that is itself a
/// file is taken as-is so callers can pass exact artifacts.
pub fn collect_rlibs(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut rlibs = Vec::new();
    for root in roots {
        collect_rlibs_into(root, &mut rlibs)?;
    }
    rlibs.sort();
    Ok(rlibs)
}

fn collect_rlibs_into(root: &Path, rlibs: &mut Vec<PathBuf>) -> Result<()> {
    let metadata = fs::symlink_metadata(root)
        .wrap_err_with(|| format!("inspecting panic-scan path {}", root.display()))?;
    if metadata.is_file() {
        rlibs.push(root.to_path_buf());
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in
            fs::read_dir(root).wrap_err_with(|| format!("reading directory {}", root.display()))?
        {
            let entry =
                entry.wrap_err_with(|| format!("reading entry under {}", root.display()))?;
            let path = entry.path();
            if path.is_dir() || path.extension().is_some_and(|ext| ext == "rlib") {
                collect_rlibs_into(&path, rlibs)?;
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

    // Builds a relocatable ELF object with one text function `func_bytes` long.
    // When `panic` is set, a relocation at offset 4 targets an undefined
    // `core::panicking` symbol, modelling a panic call inside the function.
    fn object_with_function(function: &str, panic: bool) -> Vec<u8> {
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
        if panic {
            let panic_symbol = object.add_symbol(Symbol {
                name: b"_ZN4core9panicking18panic_bounds_check17hababababababababE".to_vec(),
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
                        symbol: panic_symbol,
                        addend: 0,
                        flags: RelocationFlags::Generic {
                            kind: RelocationKind::PltRelative,
                            encoding: RelocationEncoding::X86Branch,
                            size: 32,
                        },
                    },
                )
                .expect("add panic relocation");
        }
        object.write().expect("serialize fixture object")
    }

    fn scan(data: &[u8], crate_token: Option<&str>) -> Vec<PanicFinding> {
        let mut findings = BTreeSet::new();
        scan_bytes(data, crate_token, &mut findings).expect("scan fixture");
        findings.into_iter().collect()
    }

    #[test]
    fn flags_function_that_calls_panic_entrypoint() {
        let bytes = object_with_function("_ZN7n_hello3get17habcdefgEhh", true);
        let findings = scan(&bytes, None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].function, "_ZN7n_hello3get17habcdefgEhh");
        assert!(findings[0].panic_entrypoint.contains("panic_bounds_check"));
    }

    #[test]
    fn clean_function_produces_no_findings() {
        let bytes = object_with_function("_ZN7n_hello5clean17habcdefgEhh", false);
        assert!(scan(&bytes, None).is_empty());
    }

    #[test]
    fn crate_filter_excludes_foreign_functions() {
        // A panic call lives in a `serde`-named function. Scoping the scan to
        // the `n_hello` crate token must drop it; the matching token keeps it.
        let bytes = object_with_function("_ZN5serde2de17habcdefgEhh", true);
        assert!(scan(&bytes, Some(&crate_token("n-hello"))).is_empty());
        assert_eq!(scan(&bytes, Some(&crate_token("serde"))).len(), 1);
    }

    #[test]
    fn crate_token_normalizes_dashes() {
        assert_eq!(crate_token("n-hello"), "7n_hello");
        assert_eq!(crate_token("serde"), "5serde");
    }
}
