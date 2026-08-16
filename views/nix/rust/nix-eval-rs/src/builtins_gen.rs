//! Generated from the fork sources; regenerate with the command below
//! (from the repo root) whenever a primop is added:
//!
//! grep -rhoE '\.name = "[^"]+"' src/libexpr/primops.cc \\
//!   src/libexpr/primops/*.cc src/libexpr/parallel-eval.cc | sort -u
//!
//! Each entry is the registered name: a leading __ means the bare
//! builtins.<name> strips it; the registered spelling is also a global.

pub static CPP_PRIMOP_NAMES: &[&str] = &[
    "__add",
    "__addDrvOutputDependencies",
    "__addErrorContext",
    "__all",
    "__any",
    "__appendContext",
    "__attrNames",
    "__attrValues",
    "__bitAnd",
    "__bitOr",
    "__bitXor",
    "__catAttrs",
    "__ceil",
    "__compareVersions",
    "__concatLists",
    "__concatMap",
    "__concatStringsSep",
    "__convertHash",
    "__deepSeq",
    "__div",
    "__elem",
    "__elemAt",
    "__exec",
    "__fetchClosure",
    "__fetchurl",
    "__filter",
    "__filterSource",
    "__findFile",
    "__floor",
    "__foldl'",
    "__fromJSON",
    "__functionArgs",
    "__genericClosure",
    "__genList",
    "__getAttr",
    "__getContext",
    "__getEnv",
    "__groupBy",
    "__hasAttr",
    "__hasContext",
    "__hashFile",
    "__hashString",
    "__head",
    "__importNative",
    "__intersectAttrs",
    "__isAttrs",
    "__isBool",
    "__isFloat",
    "__isFunction",
    "__isInt",
    "__isList",
    "__isPath",
    "__isString",
    "__length",
    "__lessThan",
    "__listToAttrs",
    "__mapAttrs",
    "__match",
    "__mul",
    "__outputOf",
    "__parallel",
    "__parseDrvName",
    "__partition",
    "__path",
    "__pathExists",
    "__readDir",
    "__readFile",
    "__readFileType",
    "__replaceStrings",
    "__seq",
    "__sort",
    "__split",
    "__splitVersion",
    "__storePath",
    "__stringLength",
    "__sub",
    "__substring",
    "__tail",
    "__toFile",
    "__toJSON",
    "__toPath",
    "__toXML",
    "__trace",
    "__traceVerbose",
    "__tryEval",
    "__typeOf",
    "__unsafeDiscardOutputDependency",
    "__unsafeDiscardStringContext",
    "__unsafeGetAttrPos",
    "__warn",
    "__wasm",
    "__zipAttrsWith",
    "abort",
    "baseNameOf",
    "break",
    "derivationStrict",
    "dirOf",
    "fetchFinalTree",
    "fetchGit",
    "fetchMercurial",
    "fetchTarball",
    "fetchTree",
    "fromTOML",
    "import",
    "isNull",
    "map",
    "placeholder",
    "recordedTreeAttr",
    "removeAttrs",
    "scopedImport",
    "throw",
    "toString",
];

/// Globals cppnix injects that are not primops (constants aside).
/// In `builtins` but not primops: constants and flake helpers cppnix binds
/// into the set directly. Present so `builtins ? currentSystem` is true, the
/// way the corpus asserts; unimplemented until something forces one.
pub static CPP_BUILTINS_EXTRA: &[&str] = &[
    "currentSystem",
    "currentTime",
    "derivation",
    "flakeRefToString",
    "getFlake",
    "langVersion",
    "nixPath",
    "nixVersion",
    "parseFlakeRef",
    "storeDir",
];

/// cppnix's `Constant{.impureOnly = true}`: a name that reaches neither
/// `builtins` nor the global scope under `pure-eval` (`eval.cc:541`).
///
/// A separate list from [`CPP_PRIMOP_GATES`] because it is a different
/// mechanism -- these are `addConstant` calls, not `RegisterPrimOp`, so the
/// gate table's `Feature`/`NativeCode`/`Never` vocabulary does not describe
/// them -- and because the condition is a *setting the evaluator already
/// has*, so unlike a feature gate this one can be answered here rather than
/// asked of the embedder.
///
/// Both spellings, because `addConstant("__currentSystem", ...)` binds the
/// `__`-prefixed name in scope and the stripped one in the set.
/// `the_impure_only_table_matches_the_cpp_sources` re-derives this from
/// `src/libexpr/primops.cc` and fails when the two drift.
///
/// Found by measurement, not by reading: a flake fixture calling
/// `builtins.currentSystem` under `nix build` *answered* on this backend and
/// raised `attribute 'currentSystem' missing` on cpp. A wrong value, not a
/// refusal, which is the one outcome the parity bar has no tolerance for.
pub static CPP_IMPURE_ONLY_CONSTANTS: &[&str] = &[
    "currentSystem",
    "__currentSystem",
    "currentTime",
    "__currentTime",
];

pub static CPP_EXTRA_GLOBALS: &[&str] = &[
    // `addConstant("__nixPath", ...)` rather than a primop, so the grep above
    // does not see it; it is a global in cppnix all the same, and its
    // `builtins` spelling is "nixPath" in CPP_BUILTINS_EXTRA.
    "__nixPath",
    "derivation",
    "import",
    "scopedImport",
    "fetchTarball",
    "fetchGit",
    "fetchMercurial",
    "fetchTree",
    "placeholder",
];

/// Why a name in [`CPP_PRIMOP_NAMES`] is not always registered.
///
/// cppnix's registration loop skips a primop whose `.experimentalFeature` is
/// off (`primops.cc:5606`), registers `__exec` and `__importNative` only when
/// `allow-unsafe-native-code-during-evaluation` is set (`primops.cc:5537`),
/// and files an `.internal` primop in `internalPrimOps` rather than in
/// `builtins` (`eval.cc:608`). A skipped primop is in neither the `builtins`
/// attrset nor the global scope, so `builtins ? name` is false and the bare
/// global is an undefined variable.
///
/// **This enum does not decide anything at run time.** Whether a gated name
/// is present is the embedder's answer, taken from cppnix's own `builtins`
/// (`eval::cpp_builtin_names`), because a fourth condition -- the
/// `libexpr:wasm` meson option, which decides whether `wasm.cc` is compiled
/// at all -- is a build fact no rule here could see. What this table decides
/// is *which* names the embedder is asked about, and the reason is what the
/// variant records.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gate {
    /// `RegisterPrimOp{ .experimentalFeature = Xp::… }`; the payload is the
    /// feature's `experimental-features` spelling.
    Feature(&'static str),
    /// Registered only under `allow-unsafe-native-code-during-evaluation`.
    NativeCode,
    /// Never reaches `builtins` or the global scope at all.
    Never,
}

/// The names [`CPP_PRIMOP_NAMES`] carries that cppnix does not always
/// register. Every other name in that list is unconditional and is advertised
/// here whatever the embedder says, so a short list cannot delete
/// `stringLength`.
///
/// Hand-maintained from the fork's own sources, because the `.name = "..."`
/// grep that generates the name list cannot see the condition beside it.
/// `the_gate_table_matches_the_cpp_sources` re-derives it from
/// `src/libexpr/**` and fails when the two drift -- which is what keeps a
/// newly-gated upstream primop from being advertised unconditionally again,
/// the exact shape of ENG-12717.
pub static CPP_PRIMOP_GATES: &[(&str, Gate)] = &[
    // `settings.enableNativeCode`, i.e.
    // `allow-unsafe-native-code-during-evaluation` (primops.cc:5537). Also
    // inside `#ifndef _WIN32`, which this crate does not model because the
    // fork does not build for Windows.
    ("__exec", Gate::NativeCode),
    ("__importNative", Gate::NativeCode),
    ("__fetchClosure", Gate::Feature("fetch-closure")),
    ("__outputOf", Gate::Feature("dynamic-derivations")),
    ("__parallel", Gate::Feature("parallel-eval")),
    ("__wasm", Gate::Feature("wasm-builtin")),
    // `flakes` implies `fetch-tree` when the setting is parsed
    // (configuration.cc:426), so this is true under either spelling; the
    // enabled set the embedder hands over has already been expanded.
    ("fetchTree", Gate::Feature("fetch-tree")),
    // `.internal = true` (fetchTree.cc:459): `addPrimOp` files it in
    // `internalPrimOps` and skips both the global scope and the set.
    ("fetchFinalTree", Gate::Never),
    // Not a `RegisterPrimOp` at all: `fetchTree.cc:53` allocates one per
    // recorded attribute at evaluation time. The generator's grep matches its
    // `.name` field all the same, which is how it came to be advertised.
    ("recordedTreeAttr", Gate::Never),
];

/// The gate on `name` as cppnix registers it, or `None` when it is
/// unconditional. `name` is the registered spelling, `__` and all.
#[must_use]
pub fn gate_of(name: &str) -> Option<Gate> {
    CPP_PRIMOP_GATES
        .iter()
        .find(|(gated, _)| *gated == name)
        .map(|(_, gate)| *gate)
}

#[cfg(test)]
mod gate_tests {
    use super::{CPP_IMPURE_ONLY_CONSTANTS, CPP_PRIMOP_GATES, CPP_PRIMOP_NAMES, Gate};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// The fork's own C++ sources, which this crate already depends on at
    /// build time (`vm.rs` includes `primops/derivation.nix` from here), so
    /// reading them in a test does not add a dependency.
    fn libexpr() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/libexpr")
    }

    /// Every `.cc` under `src/libexpr`, recursively.
    fn cc_sources(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                cc_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "cc")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push(text);
            }
        }
    }

    /// `Xp::Tag -> "feature-name"`, from the table that defines both.
    fn feature_names() -> BTreeMap<String, String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../src/libutil/experimental-features.cc");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            !text.is_empty(),
            "cannot read {}, so this test would pass by finding nothing",
            path.display()
        );
        let mut map = BTreeMap::new();
        let mut tag: Option<String> = None;
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix(".tag = Xp::") {
                tag = Some(rest.trim_end_matches(',').to_owned());
            } else if let Some(rest) = line.trim().strip_prefix(".name = \"")
                && let Some(name) = rest.split('"').next()
                && let Some(t) = tag.take()
            {
                map.insert(t, name.to_owned());
            }
        }
        map
    }

    /// `.name = "x"` at the start of a line, with or without the `{` that
    /// opens the struct literal on the same line (`wasm.cc:689` writes
    /// `{.name = "__wasm",`, and missing it made this scan report that
    /// cppnix does not gate `__wasm`).
    fn quoted_after(line: &str, prefix: &str) -> Option<String> {
        let trimmed = line.trim();
        let trimmed = trimmed.strip_prefix('{').unwrap_or(trimmed);
        let rest = trimmed.trim_start().strip_prefix(prefix)?;
        Some(rest.split('"').next()?.to_owned())
    }

    /// What the C++ says, as `(registered name, gate)` pairs: a
    /// `.experimentalFeature` or an `.internal` marker attaches to the
    /// nearest `.name` above it, which is the shape of every one of these
    /// struct literals.
    fn gates_from_cpp() -> BTreeMap<String, Gate> {
        let features = feature_names();
        let mut sources = Vec::new();
        cc_sources(&libexpr(), &mut sources);
        assert!(
            !sources.is_empty(),
            "found no C++ sources under {}, so this test would pass vacuously",
            libexpr().display()
        );
        let mut found = BTreeMap::new();
        for text in &sources {
            let mut name: Option<String> = None;
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(n) = quoted_after(line, ".name = \"") {
                    name = Some(n);
                } else if let Some(rest) = trimmed.strip_prefix(".experimentalFeature = Xp::") {
                    let tag = rest.trim_end_matches(&['}', ')', ';', ','][..]).trim();
                    let feature = features
                        .get(tag)
                        .cloned()
                        .unwrap_or_else(|| format!("<unknown Xp::{tag}>"));
                    if let Some(n) = &name {
                        found.insert(
                            n.clone(),
                            Gate::Feature(Box::leak(feature.into_boxed_str())),
                        );
                    }
                } else if trimmed.starts_with(".internal = true")
                    && let Some(n) = &name
                {
                    found.insert(n.clone(), Gate::Never);
                }
            }
        }
        // The two primops registered by hand behind
        // `if (settings.enableNativeCode)` rather than through
        // `RegisterPrimOp`; the block is short and closes at its own indent.
        let primops = std::fs::read_to_string(libexpr().join("primops.cc")).unwrap_or_default();
        let mut in_block = false;
        for line in primops.lines() {
            if line.contains("if (settings.enableNativeCode)") {
                in_block = true;
                continue;
            }
            if in_block {
                if line == "    }" {
                    in_block = false;
                } else if let Some(n) = quoted_after(line, ".name = \"") {
                    found.insert(n, Gate::NativeCode);
                }
            }
        }
        found
    }

    /// [`CPP_PRIMOP_GATES`] says what cppnix's own sources say.
    ///
    /// Not covered, deliberately: `recordedTreeAttr`, which is not a
    /// `RegisterPrimOp` at all but a `PrimOp` allocated per recorded
    /// attribute at evaluation time (`fetchTree.cc:53`). Nothing in the
    /// source marks it, so nothing here can derive it; it is a hand-written
    /// row and the assertion below skips it by name rather than pretending
    /// to check it.
    #[test]
    fn the_gate_table_matches_the_cpp_sources() {
        const DERIVED_BY_HAND: &[&str] = &["recordedTreeAttr"];
        let cpp = gates_from_cpp();
        assert!(
            cpp.len() >= 6,
            "derived only {} gates from the C++ sources, which is fewer than \
             the fork is known to carry; the scan is broken, not the table: {cpp:?}",
            cpp.len()
        );

        // Every gate the C++ declares for a name this crate advertises is in
        // the table, spelled the same way.
        for (name, gate) in &cpp {
            if !CPP_PRIMOP_NAMES.contains(&name.as_str()) {
                continue;
            }
            assert_eq!(
                super::gate_of(name),
                Some(*gate),
                "cppnix gates {name} as {gate:?}; CPP_PRIMOP_GATES says \
                 {:?}. A name registered behind a feature that this table \
                 leaves unconditional is advertised in `builtins` when \
                 cppnix hides it, which is ENG-12717.",
                super::gate_of(name)
            );
        }

        // And the table invents nothing.
        for (name, gate) in CPP_PRIMOP_GATES {
            if DERIVED_BY_HAND.contains(name) {
                continue;
            }
            assert_eq!(
                cpp.get(*name),
                Some(gate),
                "CPP_PRIMOP_GATES gates {name} as {gate:?}, which the C++ \
                 sources do not say; a name hidden here that cppnix \
                 registers is the same divergence in the other direction"
            );
        }
    }

    /// [`CPP_IMPURE_ONLY_CONSTANTS`] is what `primops.cc` says it is.
    ///
    /// Derived from the source rather than trusted, for the reason the gate
    /// table is: the list decides whether a name is in `builtins` under
    /// `pure-eval`, and a name missing from it answers where cppnix has no
    /// value at all. That is how `currentSystem` came to be served under a
    /// flake build.
    ///
    /// The scan looks for an `addConstant("...", ...)` call whose argument
    /// list carries `.impureOnly = true`, and the table is expected to hold
    /// both spellings of each: the `__`-prefixed name is what `addConstant`
    /// binds in scope and the stripped one is the `builtins` member.
    #[test]
    fn the_impure_only_table_matches_the_cpp_sources() {
        let text = match std::fs::read_to_string(libexpr().join("primops.cc")) {
            Ok(text) => text,
            Err(e) => unreachable!("cannot read primops.cc: {e}"),
        };
        let mut found: Vec<String> = Vec::new();
        for chunk in text.split("addConstant(").skip(1) {
            // One call's arguments: everything up to the terminator
            // `addConstant` calls are written with in this file.
            let Some((call, _)) = chunk.split_once("});") else {
                continue;
            };
            if !call.contains(".impureOnly = true") {
                continue;
            }
            let Some(rest) = call.trim_start().strip_prefix('"') else {
                continue;
            };
            let Some((name, _)) = rest.split_once('"') else {
                continue;
            };
            found.push(name.to_owned());
            if let Some(stripped) = name.strip_prefix("__") {
                found.push(stripped.to_owned());
            }
        }
        assert!(
            found.len() >= 4,
            "the scan found only {found:?} impure-only constants in primops.cc,              which is fewer than the fork is known to carry; the scan is              broken, not the table, and a broken scan here reports agreement"
        );
        found.sort();
        found.dedup();
        let mut declared: Vec<String> = CPP_IMPURE_ONLY_CONSTANTS
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        declared.sort();
        assert_eq!(
            found, declared,
            "CPP_IMPURE_ONLY_CONSTANTS and primops.cc disagree. A name cppnix              marks impureOnly and this list omits is served under pure-eval              where cppnix has no value; a name here that cppnix does not mark              is deleted from `builtins` where cppnix has one."
        );
    }

    /// Every gated name is a name that exists. A typo here would silently
    /// gate nothing.
    #[test]
    fn every_gated_name_is_a_primop_name() {
        for (name, _) in CPP_PRIMOP_GATES {
            assert!(
                CPP_PRIMOP_NAMES.contains(name),
                "{name} is gated but is not in CPP_PRIMOP_NAMES, so the gate \
                 applies to nothing"
            );
        }
    }
}
