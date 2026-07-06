# `nix run .#rebase-patches [-- <name>]`: regenerate a de-forked package's
# `patches/` series when its upstream base moves, by round-tripping through a
# real `git rebase`. The patch folder is a serialization of a git branch: fetch
# the old base (from the committed flake.lock) and the new base (from the
# working-tree flake.lock), replay the series onto the old base with `git am`,
# `git rebase --onto <new> <old>` (git's 3-way merge absorbs shifted line
# numbers and drifted context; mergiraf resolves structural conflicts), then
# re-export with `git format-patch` so the files come back with fresh context,
# deterministic bytes, and authorship/messages preserved.
#
# After a successful rebase (and via the `dag` subcommand on demand) the tool
# regenerates the `dag.json` dependency graph next to each `patches/` folder,
# since a rebase can change which patches depend on which. The DAG derivation and
# the `patch-dag-<name>` invariant check share one owner: packages/rebase-patches/
# dag-lib.nu.
#
# No fallbacks: an unresolved conflict stops loudly, printing the scratch repo
# path and the conflicting patch, for a human to resolve once and re-export.
# Committed rerere: a resolution cache per fork package (packages/<name>/rerere/,
# git rr-cache format) is seeded into the scratch repo before rebasing and
# exported back after a manual resolution, so a conflict resolved once replays on
# later runs. Every replayed resolution is printed loudly (patch + rr-cache key)
# so nothing lands silently; the re-exported patches remain the resolution of
# record and the flake checks are the correctness gate.
#
# The fork-package mapping (input name, upstream URL, patch dir) is data from
# lib/fork-packages.nix, rendered to JSON and baked in as a store path, so the
# script body hardcodes no per-package coordinates. A downstream repo (e.g. ix)
# that keeps its own fork mapping + patches reuses this one tool by pointing it
# at its list: `nix run <index>#rebase-patches -- --mapping <its-fork.json>
# [<name>]`, run from its repo root so `patchDir` and `flake.lock` resolve
# there. One tool, parameterized by data, never copied per repo.
{
  ix,
  formats,
  writeNushellApplication,
  git,
  mergiraf,
}: let
  # Fork-package mapping from the single source of truth (lib/fork-packages.nix,
  # surfaced as `ix.forkPackages`), rendered to JSON and baked in as a store
  # path so the script body hardcodes no per-package coordinates.
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  # Shared DAG library (derivation + invariant verification), baked in as a store
  # path and sourced by both this tool and the `patch-dag-<name>` flake check so
  # the two can never disagree on how a DAG is derived or validated.
  dagLib = ./dag-lib.nu;
in
  writeNushellApplication {
    name = "rebase-patches";
    meta = {
      description = "Regenerate a de-forked package's patch series via a real git rebase when its upstream base moves, and its dependency DAG";
      mainProgram = "rebase-patches";
    };
    # git for the rebase round-trip; mergiraf as the syntax-aware merge driver.
    # No pinned nix: flake.lock is read as plain JSON, so no nix invocation.
    runtimeInputs = [
      git
      mergiraf
    ];
    text = ''
      # nu
      # Run from the repo root: `nix run .#rebase-patches [-- <name>]`.
      use ${dagLib} *

      const fork_data = "${forkData}"

      # Set up a scratch git repo for `fork`: mergiraf merge driver, zdiff3
      # conflict style, and committed rerere seeded from packages/<name>/rerere/.
      # Returns the scratch path. One owner for scratch setup so `rebase one` and
      # the DAG regen path build identical trees.
      def "scratch init" [fork: record]: nothing -> string {
        let scratch = (mktemp --directory --tmpdir $"rebase-patches-($fork.name).XXXXXX")

        # mergiraf registered as the merge driver for the languages it supports
        # (git reads merge.conflictStyle + the `merge` gitattribute); zdiff3 gives
        # readable textual conflict markers when mergiraf declines. The
        # `* merge=mergiraf` mapping goes in `.git/info/attributes`, NOT a
        # worktree `.gitattributes`: an untracked worktree file would collide with
        # a tracked `.gitattributes` in the fetched upstream tree on checkout.
        git -C $scratch init --quiet
        git -C $scratch config merge.conflictStyle zdiff3
        # rerere replays a resolution the moment the same conflict recurs. The
        # committed cache (seeded below) makes it earn its keep for conflicts that
        # recur across different DAG branches or repeated upstream churn: the same
        # textual clash resolved once is replayed on every later rebase.
        git -C $scratch config rerere.enabled true
        git -C $scratch config "merge.mergiraf.name" "mergiraf syntax-aware merge"
        git -C $scratch config "merge.mergiraf.driver" "mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L"
        mkdir ($scratch | path join ".git" "info")
        "* merge=mergiraf\n" | save --force ($scratch | path join ".git" "info" "attributes")

        # Seed the committed rerere cache so prior resolutions replay. We snapshot
        # the seeded key set so the export step can tell which entries are new.
        let rr_committed = ($fork.patchDir | path expand | path join "rerere")
        let rr_scratch = ($scratch | path join ".git" "rr-cache")
        if ($rr_committed | path exists) {
          cp --recursive $rr_committed $rr_scratch
          print $"(ansi cyan)rebase-patches: ($fork.name): seeded committed rerere cache from ($fork.patchDir)/rerere(ansi reset)"
        }
        $scratch
      }

      # Export rerere entries created or touched during the rebase back to the
      # committed cache. Only entries with a recorded resolution
      # (postimage/preimage present) are exported; transient in-run misses are
      # not persisted. Loud so a new committed resolution is never silent.
      def "rerere export" [fork: record, scratch: string] {
        let rr_scratch = ($scratch | path join ".git" "rr-cache")
        if not ($rr_scratch | path exists) { return }
        let rr_committed = ($fork.patchDir | path expand | path join "rerere")
        # Keep only directories that hold a resolution (a `postimage`). A bare
        # `preimage` with no resolution is an unresolved conflict we must not
        # persist as if it were a fix.
        let resolved = (
          ls $rr_scratch
          | where type == dir
          | where {|e| ($e.name | path join "postimage") | path exists }
          | get name
          | each {|p| $p | path basename }
        )
        if ($resolved | is-empty) { return }
        mkdir $rr_committed
        for key in $resolved {
          # Replace, never merge: nushell's `cp --recursive src dest` NESTS src
          # under dest when dest already exists, so re-exporting a recurring key
          # would corrupt the committed cache (key/key/...). Clear it first.
          let dest = ($rr_committed | path join $key)
          if ($dest | path exists) { rm --recursive --force $dest }
          cp --recursive ($rr_scratch | path join $key) $dest
        }
        print $"(ansi yellow)rebase-patches: ($fork.name): exported (($resolved | length)) rerere resolution(s) to ($fork.patchDir)/rerere: (($resolved) | str join ', ')(ansi reset)"
      }

      # Print every resolution rerere REPLAYED during the rebase, keyed by the
      # patch (rebase step) it fired on. Reads git's own trace: rerere logs
      # "Recorded resolution for ..." on record and "Resolved '<file>' using
      # previous resolution." on replay. We surface the replays so nothing lands
      # silently.
      def "rerere report-replays" [fork: record, scratch: string, log: string] {
        let replays = (
          $log
          | lines
          | where {|l| $l | str contains "using previous resolution" }
        )
        if ($replays | is-empty) { return }
        print $"(ansi yellow)rebase-patches: ($fork.name): rerere REPLAYED (($replays | length)) previously-recorded resolution(s):(ansi reset)"
        for r in $replays {
          print $"  ($r | str trim)"
        }
        print $"(ansi yellow)rebase-patches: ($fork.name): review these replayed hunks; the re-exported patches are the resolution of record.(ansi reset)"
      }

      # Regenerate dag.json next to a fork package's patches. Derives the sparse
      # dependency DAG by apply-tests against `base` in a fresh scratch repo, then
      # writes the deterministic bytes. Shared by `rebase one` (post-rebase) and
      # the standalone `dag` subcommand.
      def "regen dag" [fork: record, base: string] {
        let patch_dir = ($fork.patchDir | path expand)
        let patch_files = (glob ($patch_dir | path join "*.patch") | sort)
        if ($patch_files | is-empty) {
          error make { msg: $"rebase-patches: ($fork.name): no *.patch files in ($patch_dir)" }
        }
        let patches = ($patch_files | each {|f| {name: ($f | path basename), file: $f} })

        let scratch = (mktemp --directory --tmpdir $"rebase-patches-dag-($fork.name).XXXXXX")
        git -C $scratch init --quiet
        # DAG derivation must be config-independent (a developer's global
        # rerere.enabled=true would silently corrupt the apply-tests).
        dag neutralize-config $scratch
        git -C $scratch fetch --quiet --filter=blob:none $fork.url $base
        git -C $scratch checkout --quiet --detach $base

        let nodes = (dag derive $scratch $base $patches)
        let doc = (dag document $base $nodes)
        (dag to-json $doc) | save --force --raw ($patch_dir | path join "dag.json")

        let edges = ($nodes | each {|n| $n.deps | length } | math sum)
        let roots = ($nodes | where {|n| ($n.deps | length) == 0 } | length)
        print $"(ansi green)rebase-patches: ($fork.name): regenerated dag.json (($patches | length)) nodes, ($edges) edges, ($roots) roots(ansi reset)"
        rm --recursive --force $scratch
      }

      # Regenerate one fork package's patch series. `old` / `new` are the base
      # revs from the committed vs working-tree flake.lock; skip when unchanged.
      def "rebase one" [fork: record, old: string, new: string] {
        print $"(ansi cyan)rebase-patches: ($fork.name): ($old) -> ($new)(ansi reset)"

        # Absolute patch dir before we cd into the scratch repo.
        let patch_dir = ($fork.patchDir | path expand)
        let patches = (glob ($patch_dir | path join "*.patch") | sort)
        if ($patches | is-empty) {
          error make { msg: $"rebase-patches: ($fork.name): no *.patch files in ($patch_dir)" }
        }

        let scratch = (scratch init $fork)

        # Blobless fetch of just the two revs we round-trip between.
        git -C $scratch fetch --quiet --filter=blob:none $fork.url $old $new
        git -C $scratch checkout --quiet --detach $old

        # Replay the committed series onto the old base: our branch, bit-identical.
        try {
          git -C $scratch am ...$patches
        } catch {
          git -C $scratch am --abort
          error make { msg: $"rebase-patches: ($fork.name): `git am` failed replaying the committed series onto the pinned base ($old); scratch repo: ($scratch)" }
        }

        # Rebase our branch onto the new base. 3-way + mergiraf absorb the
        # mechanical drift; a real semantic collision aborts here. Capture the
        # combined output so we can surface any rerere replays.
        let rebased = (do { git -C $scratch rebase --onto $new $old } | complete)
        let rebase_log = ($rebased.stdout + $rebased.stderr)
        if $rebased.exit_code != 0 {
          rerere report-replays $fork $scratch $rebase_log
          let conflict = (
            git -C $scratch diff --name-only --diff-filter=U
            | lines
            | str join ", "
          )
          # Persist any resolutions recorded so far so a human's earlier fixes are
          # not lost, then leave the scratch repo conflicted for them to finish.
          rerere export $fork $scratch
          error make { msg: $"rebase-patches: ($fork.name): rebase onto ($new) hit an unresolved conflict in [($conflict)]; resolve in the scratch repo then `git rebase --continue` and re-run, or fix the offending patch. Scratch repo: ($scratch)" }
        }

        # Loudly report and persist any rerere resolutions that fired.
        rerere report-replays $fork $scratch $rebase_log
        rerere export $fork $scratch

        # Wipe and re-serialize with the deterministic flag set: fresh context
        # and line numbers, zeroed commit ids, no signature/version trailer, so
        # the PR diff shows only real drift.
        rm --recursive --force ...(glob ($patch_dir | path join "*.patch"))
        git -C $scratch format-patch --zero-commit --no-signature --no-stat -N -o $patch_dir $"($new)..HEAD"
        print $"(ansi green)rebase-patches: ($fork.name): regenerated (glob ($patch_dir | path join '*.patch') | length) patches in ($fork.patchDir)(ansi reset)"

        rm --recursive --force $scratch

        # A rebase can change dependencies, so the DAG is regenerated against the
        # new base at the end of every run.
        regen dag $fork $new
      }

      # `dag` subcommand: regenerate dag.json for one or all fork packages against
      # the currently-pinned base (working-tree flake.lock), without a rebase.
      def "main dag" [
        name?: string  # one fork package (codex | btop | clippy); all if omitted
        --mapping: string # fork-package JSON to drive (default: index's baked-in list)
      ] {
        let forks = (fork select $name $mapping)
        let new_lock = (open --raw flake.lock | from json)
        for fork in $forks {
          let base = ($new_lock.nodes | get $fork.input | get locked | get rev)
          regen dag $fork $base
        }
      }

      # The fork-package mapping to drive: the caller-supplied `--mapping` path
      # (a downstream repo pointing this one tool at its own fork list, run from
      # its repo root so `patchDir`/`flake.lock` resolve there) else index's own
      # baked-in list. One tool, parameterized by data, never copied.
      def "mapping path" [override?: string]: nothing -> string {
        if $override == null { $fork_data } else { $override }
      }

      # Resolve the selected fork records from an optional name against `mapping`.
      def "fork select" [name?: string, mapping?: string]: nothing -> list<record> {
        let forks = (open (mapping path $mapping))
        if $name == null { return $forks }
        let hit = ($forks | where name == $name)
        if ($hit | is-empty) {
          error make { msg: $"rebase-patches: no fork package named ($name); known: (($forks | get name) | str join ', ')" }
        }
        $hit
      }

      def main [
        name?: string  # one fork package (codex | btop | clippy | mesa); all changed if omitted
        --mapping: string # fork-package JSON to drive (default: index's baked-in list)
      ] {
        let selected = (fork select $name $mapping)

        # Old base from the committed flake.lock, new base from the working
        # tree. `flake.lock` has no nushell-recognized extension, so parse JSON
        # explicitly rather than relying on `open`'s content auto-detection.
        let old_lock = (git show HEAD:flake.lock | from json)
        let new_lock = (open --raw flake.lock | from json)

        mut did_any = false
        for fork in $selected {
          let old_rev = ($old_lock.nodes | get $fork.input | get locked | get rev)
          let new_rev = ($new_lock.nodes | get $fork.input | get locked | get rev)
          if $old_rev == $new_rev {
            print $"rebase-patches: ($fork.name): base unchanged (($old_rev)); nothing to do"
            continue
          }
          rebase one $fork $old_rev $new_rev
          $did_any = true
        }
        if not $did_any {
          print "rebase-patches: no fork input moved; patches are up to date"
        }
      }
    '';
  }
