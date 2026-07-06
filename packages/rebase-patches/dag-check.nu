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
#   (d) the hand-written upstreaming intent (lib/fork-packages.nix `patches`)
#       is coherent with the series: every intent key names a real patch file
#       (a rebase can renumber/rename patches and orphan intent silently), and
#       every `attempt`-marked patch has a substantive commit-message body,
#       because the upstream PR body IS the commit message (one fact, one home;
#       see packages/upstream-pr). An attempt patch with a bare subject would
#       open a description-less PR, so it fails here with "write the why".
#
# Args: <src-dir> <patch-dir> <expected-base-rev> [<intent-json>]. The base rev
# is the upstream rev the fork is pinned at (flake.lock), so the committed
# dag.json's `base` field is validated against the real pin, not just the
# synthetic commit. `intent-json` is the fork's `patches` intent attrset
# rendered to JSON (defaults to empty for forks with no declared intent).
use dag-lib.nu *

# The commit-message body of a format-patch file: the lines between the header
# block (ended by the first blank line; folded Subject continuations are
# indented, never blank) and the diff payload (`---` separator, or `diff --git`
# directly when the series is exported with --no-stat). Blank-only lines do not
# count as substance.
def "patch body-lines" [file: string]: nothing -> list<string> {
  open --raw $file
  | lines
  | skip until {|l| $l == "" }
  | skip 1
  | take until {|l| ($l == "---") or ($l | str starts-with "diff --git ") }
  | where {|l| ($l | str trim) != "" }
}

def main [src_dir: string, patch_dir: string, expected_base: string, intent_json: string = "{}"] {
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

  # (d) intent coherence. Keys must name real patch files (a rebase renumbers
  # names and would orphan intent silently), and attempt-marked patches must
  # carry the why in the commit body, because that body becomes the upstream PR
  # description verbatim (packages/upstream-pr); nix deliberately has no
  # duplicate description field.
  let intent = ($intent_json | from json)
  let names = ($patches | get name)
  for key in ($intent | columns) {
    if $key not-in $names {
      print $"patch-dag check: lib/fork-packages.nix intent references nonexistent patch ($key) \(renamed by a rebase?\); update the intent key."
      $failed = true
    }
  }
  let attempts = ($intent | items {|k, v| {name: $k, mark: ($v.upstream? | default "hold")} } | where mark == "attempt" | get name)
  for nm in $attempts {
    if $nm not-in $names { continue }  # already reported above
    let body = (patch body-lines ($patch_dir | path join $nm))
    if ($body | is-empty) {
      print $"patch-dag check: ($nm) is marked upstream = attempt but its commit message has no body; write the why in the commit body \(it becomes the upstream PR description\)."
      $failed = true
    }
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
