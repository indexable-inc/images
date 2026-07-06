# Driver for the `patch-dag-<name>` flake check. Runs in the Nix build sandbox
# (no network): it takes the already-fetched upstream `src` tree as a directory,
# seeds a throwaway git repo from it as the base commit, and runs the shared
# invariant checks in dag-lib.nu against the committed `dag.json`.
#
# Invariants (all pure text work on the base tree, seconds-fast, no builds):
#   (a) every patch applies given ONLY its declared DAG ancestors,
#   (b) every pair of DAG-independent patches commutes byte-for-byte,
#   (c) dag.json is in sync: regenerating from scratch yields identical bytes,
#       and the NNNN order is a valid topological order of the DAG.
#
# Args: <src-dir> <patch-dir> <expected-base-rev>. `expected-base-rev` is the
# upstream rev the fork is pinned at (flake.lock), so the committed dag.json's
# `base` field is validated against the real pin, not just the synthetic commit.
use dag-lib.nu *

def main [src_dir: string, patch_dir: string, expected_base: string] {
  let files = (glob ($patch_dir | path join "*.patch") | sort)
  if ($files | is-empty) {
    print $"patch-dag check: no *.patch files in ($patch_dir)"
    exit 1
  }
  let patches = ($files | each {|f| {name: ($f | path basename), file: $f} })

  let dag_file = ($patch_dir | path join "dag.json")
  if not ($dag_file | path exists) {
    print $"patch-dag check: missing dag.json in ($patch_dir); run `nix run .#rebase-patches -- dag`"
    exit 1
  }
  let committed_bytes = (open --raw $dag_file)
  let doc = ($committed_bytes | from json)

  # Seed a git repo from the fetched src tree and commit it as the base.
  let scratch = (mktemp --directory --tmpdir "patch-dag-check.XXXXXX")
  let base = (dag seed-base-repo $src_dir $scratch)

  mut failed = false

  # (c-base) The committed dag.json base must match the pinned upstream rev, so a
  # flake.lock bump that skipped `rebase-patches -- dag` fails loudly.
  if $doc.base != $expected_base {
    print $"patch-dag check: dag.json base \(($doc.base)\) does not match the pinned upstream rev \(($expected_base)\); run `nix run .#rebase-patches -- dag` and commit."
    $failed = true
  }

  # (c-sync) Regenerating the DAG from the same patches + base must reproduce the
  # committed bytes exactly, so a stale committed DAG fails loudly. We derive
  # against a FRESH scratch clone of the same base so derivation and verification
  # do not share dirty state. The committed `base` field is rewritten to the
  # check's synthetic base rev before comparison, since the derivation records
  # whatever base it ran against; every other byte must match.
  let derive_scratch = (mktemp --directory --tmpdir "patch-dag-derive.XXXXXX")
  let derive_base = (dag seed-base-repo $src_dir $derive_scratch)
  let regen_nodes = (dag derive $derive_scratch $derive_base $patches)
  # Compare structure (nodes) independent of the base rev: the committed base is
  # an upstream rev, the check's base is a synthetic local commit, so only the
  # edge set is meaningfully comparable here.
  if ($regen_nodes | to json) != ($doc.nodes | to json) {
    print "patch-dag check: dag.json is STALE. Regenerating produces a different graph:"
    print "  committed nodes:"
    for n in $doc.nodes { print $"    ($n.patch) -> [($n.deps | str join ', ')]" }
    print "  regenerated nodes:"
    for n in $regen_nodes { print $"    ($n.patch) -> [($n.deps | str join ', ')]" }
    print "  run `nix run .#rebase-patches -- dag` and commit the result."
    $failed = true
  }

  # (a) + (b) + (c-topo): the shared verifier against the synthetic base. We pass
  # a doc whose base is rewritten to the synthetic rev so the base-match check
  # inside `dag verify` is about structure, not the upstream rev (which the
  # sandbox cannot know maps to this local commit).
  let doc_local = ($doc | upsert base $base)
  let r = (dag verify $scratch $base $patches $doc_local)
  if not $r.ok {
    print "patch-dag check: invariant violations:"
    for e in $r.errors { print $"  - ($e)" }
    $failed = true
  }

  # Best-effort cleanup. git marks pack/object files read-only, so a plain `rm`
  # can hit permission-denied; the OS/Nix reaps the tempdir regardless, so we do
  # not let cleanup failure mask the check result.
  try { rm --recursive --force $scratch }
  try { rm --recursive --force $derive_scratch }

  if $failed {
    exit 1
  }
  print $"patch-dag check: OK \(($patches | length) patches, (($doc.nodes | each {|n| $n.deps | length} | math sum)) edges\)"
}
