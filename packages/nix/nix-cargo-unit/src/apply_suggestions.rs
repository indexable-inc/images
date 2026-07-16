//! Apply `MachineApplicable` compiler suggestions to sources on disk.
//!
//! Per-unit clippy invokes `clippy-driver` directly with a rendered rustc
//! argv (dependencies are prebuilt store-path rlibs), so cargo's `--fix`
//! machinery, which lives inside cargo and drives rustc through
//! `RUSTC_WRAPPER`, cannot run in a unit sandbox. This subcommand replicates
//! the applying half of `cargo fix` on the same `rustfix` library cargo
//! uses: parse `--error-format=json` diagnostics, collect `MachineApplicable`
//! suggestions, and rewrite the affected files in place. The per-package fix
//! derivation loops driver + apply to a fixpoint (#3434), mirroring cargo
//! fix's re-evaluate-until-quiet design (see `rustfix::apply_suggestions`).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use color_eyre::eyre::WrapErr as _;
use rustfix::diagnostics::Diagnostic;
use rustfix::{CodeFix, Filter, Suggestion, collect_suggestions};

/// Counters for one apply pass. `applied` drives the caller's fixpoint loop
/// (another compile is worthwhile only if something changed); the rest exist
/// for the human-readable summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Suggestions written to disk in this pass.
    pub applied: usize,
    /// Suggestions skipped because an earlier replacement in the same pass
    /// overlapped them; the next driver run re-emits them against the
    /// rewritten code (or not at all). Mirrors cargo fix's handling of
    /// `rustfix::Error::AlreadyReplaced`.
    pub deferred: usize,
    /// Suggestions skipped because they edit files outside the writable
    /// source root (macro-expanded spans in read-only dependency sources) or
    /// span multiple files (cargo fix skips those rather than apply half).
    pub skipped: usize,
}

/// Applies `MachineApplicable` suggestions from `--error-format=json`
/// diagnostics (one JSON object per line) to files under `source_root`.
pub fn apply_diagnostics(source_root: &Path, diagnostics: &str) -> color_eyre::Result<Outcome> {
    // No error-code filter: whatever the driver was configured to emit
    // (manifest lint flags plus rustc's own lints) is what gets fixed,
    // exactly like `cargo clippy --fix`.
    let all_codes: HashSet<String> = HashSet::new();
    let mut outcome = Outcome::default();
    // Group suggestions by the file they edit; BTreeMap keeps the pass
    // deterministic so identical diagnostics always produce identical bytes.
    let mut by_file: BTreeMap<String, Vec<Suggestion>> = BTreeMap::new();

    for line in diagnostics.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // The driver's stderr is one JSON object per line under
        // --error-format=json; anything else (an ICE backtrace, a linker
        // note) is passed through instead of silently swallowed.
        let Ok(diagnostic) = serde_json::from_str::<Diagnostic>(line) else {
            eprintln!("{line}");
            continue;
        };
        let Some(suggestion) =
            collect_suggestions(&diagnostic, &all_codes, Filter::MachineApplicableOnly)
        else {
            continue;
        };
        let files: HashSet<&str> = suggestion
            .solutions
            .iter()
            .flat_map(|solution| &solution.replacements)
            .map(|replacement| replacement.snippet.file_name.as_str())
            .collect();
        // A suggestion spanning multiple files cannot be applied atomically
        // by the per-file rewrite below; cargo fix skips these too.
        let [file] = *files.iter().collect::<Vec<_>>().as_slice() else {
            outcome.skipped += 1;
            continue;
        };
        // Only the writable copy of the crate's own source is fixable: a
        // macro-expanded span can point into a read-only dependency source
        // in the store, which is not this package's finding to rewrite.
        if !Path::new(file).starts_with(source_root) {
            outcome.skipped += 1;
            continue;
        }
        by_file
            .entry((*file).to_owned())
            .or_default()
            .push(suggestion);
    }

    for (file, suggestions) in &by_file {
        let code = std::fs::read_to_string(file)
            .wrap_err_with(|| format!("reading source to fix: {file}"))?;
        let mut fix = CodeFix::new(&code);
        // Reverse order matches rustfix::apply_suggestions and cargo fix:
        // later suggestions are applied first so an overlap surfaces as
        // AlreadyReplaced instead of corrupting earlier spans.
        for suggestion in suggestions.iter().rev() {
            match fix.apply(suggestion) {
                Ok(()) => outcome.applied += 1,
                Err(rustfix::Error::AlreadyReplaced { .. }) => outcome.deferred += 1,
                Err(err) => {
                    return Err(err)
                        .wrap_err_with(|| format!("applying clippy suggestion to {file}"));
                }
            }
        }
        if fix.modified() {
            let fixed = fix
                .finish()
                .wrap_err_with(|| format!("rendering fixed source for {file}"))?;
            std::fs::write(file, fixed)
                .wrap_err_with(|| format!("writing fixed source: {file}"))?;
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal rustc JSON diagnostic with one `MachineApplicable` child
    /// suggestion replacing `range` in `file` with `replacement`.
    fn diagnostic_json(file: &str, range: std::ops::Range<usize>, replacement: &str) -> String {
        serde_json::json!({
            "message": "this expression creates a copy",
            "code": {"code": "clippy::clone_on_copy", "explanation": null},
            "level": "warning",
            "spans": [],
            "children": [{
                "message": "try removing the `clone` call",
                "code": null,
                "level": "help",
                "spans": [{
                    "file_name": file,
                    "byte_start": range.start,
                    "byte_end": range.end,
                    "line_start": 1,
                    "line_end": 1,
                    "column_start": range.start + 1,
                    "column_end": range.end + 1,
                    "is_primary": true,
                    "text": [],
                    "label": null,
                    "suggested_replacement": replacement,
                    "suggestion_applicability": "MachineApplicable",
                    "expansion": null
                }],
                "children": [],
                "rendered": null
            }],
            "rendered": "rendered message"
        })
        .to_string()
    }

    #[test]
    fn applies_suggestion_inside_root_and_skips_outside() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let file = tmp.path().join("lib.rs");
        std::fs::write(&file, "let x = y.clone();\n").expect("write fixture source");

        let inside = diagnostic_json(&file.display().to_string(), 8..17, "y");
        let outside = diagnostic_json("/nix/store/dep-src/lib.rs", 0..1, "z");
        let outcome = apply_diagnostics(tmp.path(), &format!("{inside}\n{outside}\n"))
            .expect("apply diagnostics");

        assert_eq!(
            outcome,
            Outcome {
                applied: 1,
                deferred: 0,
                skipped: 1,
            }
        );
        let fixed = std::fs::read_to_string(&file).expect("read fixed source");
        assert_eq!(fixed, "let x = y;\n");
    }

    #[test]
    fn overlapping_suggestions_defer_to_the_next_pass() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let file = tmp.path().join("lib.rs");
        std::fs::write(&file, "let x = y.clone();\n").expect("write fixture source");

        let path = file.display().to_string();
        let first = diagnostic_json(&path, 8..17, "y");
        let second = diagnostic_json(&path, 8..17, "*y");
        let outcome = apply_diagnostics(tmp.path(), &format!("{first}\n{second}\n"))
            .expect("apply diagnostics");

        assert_eq!(outcome.applied, 1);
        assert_eq!(outcome.deferred, 1);
    }

    #[test]
    fn non_json_lines_pass_through_without_failing() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let outcome = apply_diagnostics(tmp.path(), "thread panicked at compiler/...\n")
            .expect("apply diagnostics");
        assert_eq!(outcome, Outcome::default());
    }
}
