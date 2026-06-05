//! Parse nix-fast-build's `--result-format json --result-file` output (the
//! `check` app writes it to `check-results.json` in cwd; check.yml uploads it
//! as an artifact).
//!
//! Each entry is one phase for one attr: `{attr, type: "EVAL" | "BUILD",
//! duration, success, error, outputs}`. We only care about the BUILD pass:
//! that is the wall-clock the rebuilt-checks list annotates.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use color_eyre::eyre::{Context as _, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ResultFile {
    results: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    attr: String,
    #[serde(rename = "type")]
    kind: String,
    duration: f64,
}

/// Map of attribute -> BUILD wall-clock seconds. EVAL entries are dropped; a
/// missing attr (substituter cache hit, never rebuilt this run) is absent
/// from the map rather than reported as zero.
pub fn load<P: AsRef<Path>>(path: P) -> Result<BTreeMap<String, f64>> {
    let path = path.as_ref();
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read nix-fast-build result file {}", path.display()))?;
    parse(&raw)
        .with_context(|| format!("parse nix-fast-build result file {}", path.display()))
}

fn parse(raw: &str) -> Result<BTreeMap<String, f64>> {
    let file: ResultFile = serde_json::from_str(raw)?;
    let mut out = BTreeMap::new();
    for entry in file.results {
        if entry.kind == "BUILD" {
            // nix-fast-build copies nix-eval-jobs' joined attr verbatim, so a
            // sharded ciChecks leaf arrives quoted (`rust-foo."doctest-..."`).
            // Normalize the same way the eval side does (see nix::normalize_attr)
            // so the timings key matches the rebuilt-check name and never carries
            // a `"` the workflow safename regex would reject.
            //
            // Later entries win on the rare retry; nix-fast-build emits a fresh
            // BUILD record per attempt.
            out.insert(crate::nix::normalize_attr(&entry.attr), entry.duration);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_eval_entries_and_keeps_build_durations() {
        let raw = r#"{
            "results": [
                {"attr": "lint", "type": "EVAL", "duration": 0.0, "success": true, "error": null},
                {"attr": "lint", "type": "BUILD", "duration": 12.5, "success": true, "error": null, "outputs": {"out": "/nix/store/abc"}},
                {"attr": "rust-test-search", "type": "BUILD", "duration": 87.3, "success": true, "error": null, "outputs": {"out": "/nix/store/def"}}
            ]
        }"#;
        let map = parse(raw).unwrap();
        assert_eq!(map.len(), 2);
        // f64 equality is forbidden by clippy::float_cmp; the BUILD durations
        // are deliberate small constants so an absolute-tolerance check is
        // sufficient and avoids the lint suppression.
        assert!((map["lint"] - 12.5).abs() < 1e-9);
        assert!((map["rust-test-search"] - 87.3).abs() < 1e-9);
    }

    // A sharded ciChecks doctest leaf reaches nix-fast-build's result file with
    // its interior segment quoted. The key is normalized to the bare dot path so
    // it matches the rebuilt-check name and passes the workflow safename regex.
    #[test]
    fn unquotes_sharded_attr_keys() {
        let raw = r#"{
            "results": [
                {"attr": "rust-foo.\"doctest-src/lib.rs - (line 12)\"", "type": "BUILD", "duration": 3.0, "success": true, "error": null, "outputs": {"out": "/nix/store/abc"}}
            ]
        }"#;
        let map = parse(raw).unwrap();
        assert_eq!(map.len(), 1);
        assert!((map["rust-foo.doctest-src/lib.rs - (line 12)"] - 3.0).abs() < 1e-9);
    }
}
