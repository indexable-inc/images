//! Driver for the `patch-dag-<name>` flake check. Runs in the Nix build
//! sandbox (no network): it takes the already-fetched upstream `src` tree as a
//! directory, seeds a throwaway git repo from it as the base commit, and runs
//! the shared invariant checks in src/dag.rs against the committed `dag.json`.
//!
//! Invariants (all pure text work on the base tree, seconds-fast, no builds):
//!   (a) every patch applies given ONLY its declared DAG ancestors,
//!   (b) every pair of DAG-independent patches commutes byte-for-byte,
//!   (c) dag.json is in sync: regenerating from scratch yields the identical
//!       graph, and the NNNN order is a valid topological order of the DAG.
//!   (d) the hand-written upstreaming intent (lib/fork-packages.nix `patches`)
//!       is coherent with the series: every intent key names a real patch file
//!       (a rebase can renumber/rename patches and orphan intent silently),
//!   (e) every patch states WHY it exists in its commit-message body. The body
//!       is the reason of record (one fact, one home): it rides the `git am` /
//!       rebase / `format-patch` round-trip, so it reaches upstream reviewers,
//!       and for attempt-marked patches it becomes the upstream PR description
//!       verbatim (see packages/upstream-pr). Attribution trailers and bare
//!       issue refs are not a reason, so a mute patch fails with "write the
//!       why".
//!
//! The expected base rev is the upstream rev the fork is pinned at
//! (flake.lock), so the committed dag.json `base` field is validated against
//! the real pin, not just the synthetic commit. The intent JSON is the fork
//! `patches` intent attrset rendered to JSON (empty for forks with no declared
//! intent).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;
use tempfile::TempDir;

use crate::dag;

/// Run the check; `Ok(false)` means violations were printed and the caller
/// must exit non-zero. Scratch cleanup is best-effort by design ([`TempDir`]
/// ignores removal errors on drop): git marks pack/object files read-only, so
/// removal can fail, the OS reaps the tempdir regardless, and cleanup failure
/// must not mask the check result.
pub(crate) fn run(src_dir: &Path, patch_dir: &Path, expected_base: &str, intent_json: &str) -> Result<bool> {
    let patches = dag::patches_in(patch_dir)?;
    if patches.is_empty() {
        println!("patch-dag check: no *.patch files in {}", patch_dir.display());
        return Ok(false);
    }

    let dag_file = patch_dir.join("dag.json");
    if !dag_file.exists() {
        println!(
            "patch-dag check: missing dag.json in {}; run `nix run .#rebase-patches -- dag`",
            patch_dir.display()
        );
        return Ok(false);
    }
    let doc: dag::Document = serde_json::from_str(
        &fs::read_to_string(&dag_file).with_context(|| format!("read {}", dag_file.display()))?,
    )
    .with_context(|| format!("parse {}", dag_file.display()))?;

    // Seed a git repo from the fetched src tree and commit it as the base.
    let scratch = TempDir::with_prefix("patch-dag-check.")?;
    let base = dag::seed_base_repo(src_dir, scratch.path())?;

    let mut failed = false;

    // (c-base) The committed dag.json base must match the pinned upstream rev,
    // so a flake.lock bump that skipped `rebase-patches dag` fails loudly.
    if doc.base != expected_base {
        println!(
            "patch-dag check: dag.json base ({}) does not match the pinned upstream rev ({expected_base}); run `nix run .#rebase-patches -- dag` and commit.",
            doc.base
        );
        failed = true;
    }

    // (c-sync) Regenerating the DAG from the same patches + base must
    // reproduce the committed graph exactly, so a stale committed DAG fails
    // loudly. We derive against a FRESH scratch clone of the same base so
    // derivation and verification do not share dirty state. Only the nodes are
    // compared: the committed `base` is an upstream rev, the check base is a
    // synthetic local commit, so only the edge set is meaningfully comparable
    // here.
    let derive_scratch = TempDir::with_prefix("patch-dag-derive.")?;
    let derive_base = dag::seed_base_repo(src_dir, derive_scratch.path())?;
    let regen_nodes = dag::derive(derive_scratch.path(), &derive_base, &patches)?;
    if regen_nodes != doc.nodes {
        println!("patch-dag check: dag.json is STALE. Regenerating produces a different graph:");
        println!("  committed nodes:");
        for node in &doc.nodes {
            println!("    {} -> [{}]", node.patch, node.deps.join(", "));
        }
        println!("  regenerated nodes:");
        for node in &regen_nodes {
            println!("    {} -> [{}]", node.patch, node.deps.join(", "));
        }
        println!("  run `nix run .#rebase-patches -- dag` and commit the result.");
        failed = true;
    }

    // (a) + (b) + (c-topo): the shared verifier against the synthetic base. We
    // pass a doc whose base is rewritten to the synthetic rev so the
    // base-match check inside `dag::verify` is about structure, not the
    // upstream rev (which the sandbox cannot know maps to this local commit).
    let doc_local = dag::Document { base: base.clone(), ..doc.clone() };
    let report = dag::verify(scratch.path(), &base, &patches, &doc_local)?;
    if !report.ok() {
        println!("patch-dag check: invariant violations:");
        for error in &report.errors {
            println!("  - {error}");
        }
        failed = true;
    }

    // (d) intent coherence: keys must name real patch files (a rebase
    // renumbers names and would orphan intent silently).
    let intent: serde_json::Map<String, Value> =
        serde_json::from_str(intent_json).context("parse the intent JSON argument")?;
    for key in intent.keys() {
        if !patches.iter().any(|p| p.name == *key) {
            println!(
                "patch-dag check: lib/fork-packages.nix intent references nonexistent patch {key} (renamed by a rebase?); update the intent key."
            );
            failed = true;
        }
    }

    // (e) every patch carries its reason inline. The commit-message body is
    // the reason of record; nix deliberately has no duplicate description
    // field (lib/fork-packages.nix `reason` explains the upstreaming STANCE,
    // not the patch), and for attempt-marked patches the body becomes the
    // upstream PR description verbatim (packages/upstream-pr). A bare subject
    // is mute both here and in an upstream reviewer inbox.
    for patch in &patches {
        if !dag::body_has_reason(&patch.file)? {
            println!(
                "patch-dag check: {} states no reason in its commit-message body; write why the patch exists in the commit body (attribution trailers and bare issue refs do not count; for attempt patches the body becomes the upstream PR description).",
                patch.name
            );
            failed = true;
        }
    }

    if failed {
        return Ok(false);
    }
    let edges: usize = doc.nodes.iter().map(|n| n.deps.len()).sum();
    println!("patch-dag check: OK ({} patches, {edges} edges)", patches.len());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;

    /// Build a plain src tree (no `.git`), a one-patch series derived from it,
    /// and the matching dag.json; returns (src, patches). Mirrors what the
    /// flake check receives: a store-path src plus the committed patch dir.
    fn check_fixture(dir: &Path, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let src = dir.join("src");
        let patch_dir = dir.join("patches");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&patch_dir).unwrap();
        fs::write(src.join("file"), "one\ntwo\n").unwrap();

        // Derive the patch from a scratch repo seeded the same way the check
        // seeds its base.
        let work = dir.join("work");
        fs::create_dir_all(&work).unwrap();
        let base = dag::seed_base_repo(&src, &work).unwrap();
        fs::write(work.join("file"), "one\npatched\n").unwrap();
        git::run(&work, &["add", "--all"]).unwrap();
        git::run(&work, &["commit", "--quiet", "-m", &format!("edit file\n\n{body}")]).unwrap();
        let out = git::utf8(&patch_dir).unwrap().to_owned();
        let range = format!("{base}..HEAD");
        git::run(
            &work,
            &["format-patch", "--zero-commit", "--no-signature", "--no-stat", "-N", "-o", &out, &range],
        )
        .unwrap();

        let patches = dag::patches_in(&patch_dir).unwrap();
        let nodes = patches
            .iter()
            .map(|p| dag::Node { patch: p.name.clone(), deps: Vec::new() })
            .collect();
        let doc = dag::document("pinned-rev", nodes);
        fs::write(patch_dir.join("dag.json"), dag::to_json(&doc).unwrap()).unwrap();
        (src, patch_dir)
    }

    #[test]
    fn passes_a_coherent_series() {
        let dir = tempfile::TempDir::new().unwrap();
        let (src, patch_dir) = check_fixture(dir.path(), "Reason: exercises the check driver.");
        assert!(run(&src, &patch_dir, "pinned-rev", "{}").unwrap());
    }

    #[test]
    fn fails_on_base_mismatch_orphan_intent_and_mute_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let (src, patch_dir) = check_fixture(dir.path(), "Refs #1");
        // Wrong pin, an intent key naming no patch, and a body with no reason:
        // each alone must fail the check; together they must too.
        let intent = r#"{"0009-nonexistent.patch": {"kind": "attempt"}}"#;
        assert!(!run(&src, &patch_dir, "some-other-rev", intent).unwrap());
    }

    #[test]
    fn fails_on_stale_committed_dag() {
        let dir = tempfile::TempDir::new().unwrap();
        let (src, patch_dir) = check_fixture(dir.path(), "Reason: exercises staleness.");
        // Corrupt the committed graph with a bogus extra node: regeneration
        // cannot reproduce it.
        let dag_file = patch_dir.join("dag.json");
        let mut doc: dag::Document =
            serde_json::from_str(&fs::read_to_string(&dag_file).unwrap()).unwrap();
        doc.nodes.push(dag::Node { patch: "9999-ghost.patch".to_owned(), deps: Vec::new() });
        fs::write(&dag_file, dag::to_json(&doc).unwrap()).unwrap();
        assert!(!run(&src, &patch_dir, "pinned-rev", "{}").unwrap());
    }
}
