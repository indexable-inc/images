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
# No fallbacks: an unresolved conflict stops loudly, printing the scratch repo
# path and the conflicting patch, for a human to resolve once and re-export.
#
# The fork-package mapping (input name, upstream URL, patch dir) is data from
# lib/fork-packages.nix, rendered to JSON and baked in as a store path, so the
# script body hardcodes no per-package coordinates.
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
in
  writeNushellApplication {
    name = "rebase-patches";
    meta = {
      description = "Regenerate a de-forked package's patch series via a real git rebase when its upstream base moves";
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
      const fork_data = "${forkData}"

      # Regenerate one fork package's patch series. `old` / `new` are the base
      # revs from the committed vs working-tree flake.lock; skip when unchanged.
      def "rebase one" [fork: record, old: string, new: string] {
        print $"(ansi cyan)rebase-patches: ($fork.name): ($old) -> ($new)(ansi reset)"

        let scratch = (mktemp --directory --tmpdir $"rebase-patches-($fork.name).XXXXXX")
        # Absolute patch dir before we cd into the scratch repo.
        let patch_dir = ($fork.patchDir | path expand)
        let patches = (glob ($patch_dir | path join "*.patch") | sort)
        if ($patches | is-empty) {
          error make { msg: $"rebase-patches: ($fork.name): no *.patch files in ($patch_dir)" }
        }

        # Scratch repo with mergiraf registered as the merge driver for the
        # languages it supports (git reads merge.conflictStyle + the `merge`
        # gitattribute); zdiff3 gives readable textual conflict markers when
        # mergiraf declines. The `* merge=mergiraf` mapping goes in
        # `.git/info/attributes`, NOT a worktree `.gitattributes`: an untracked
        # worktree file would collide with a tracked `.gitattributes` in the
        # fetched upstream tree on checkout. mergiraf decides per-file whether it
        # has a grammar.
        git -C $scratch init --quiet
        git -C $scratch config merge.conflictStyle zdiff3
        git -C $scratch config "merge.mergiraf.name" "mergiraf syntax-aware merge"
        git -C $scratch config "merge.mergiraf.driver" "mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L"
        mkdir ($scratch | path join ".git" "info")
        "* merge=mergiraf\n" | save --force ($scratch | path join ".git" "info" "attributes")

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
        # mechanical drift; a real semantic collision aborts here.
        let rebased = (do { git -C $scratch rebase --onto $new $old } | complete)
        if $rebased.exit_code != 0 {
          let conflict = (
            git -C $scratch diff --name-only --diff-filter=U
            | lines
            | str join ", "
          )
          # Leave the scratch repo in the conflicted state for a human.
          error make { msg: $"rebase-patches: ($fork.name): rebase onto ($new) hit an unresolved conflict in [($conflict)]; resolve in the scratch repo then `git rebase --continue` and re-run, or fix the offending patch. Scratch repo: ($scratch)" }
        }

        # Wipe and re-serialize with the deterministic flag set: fresh context
        # and line numbers, zeroed commit ids, no signature/version trailer, so
        # the PR diff shows only real drift.
        rm --recursive --force ...(glob ($patch_dir | path join "*.patch"))
        git -C $scratch format-patch --zero-commit --no-signature --no-stat -N -o $patch_dir $"($new)..HEAD"
        print $"(ansi green)rebase-patches: ($fork.name): regenerated (glob ($patch_dir | path join '*.patch') | length) patches in ($fork.patchDir)(ansi reset)"

        rm --recursive --force $scratch
      }

      def main [
        name?: string # one fork package (codex | btop | clippy); all changed if omitted
      ] {
        let forks = (open $fork_data)
        let selected = if $name != null {
          let hit = ($forks | where name == $name)
          if ($hit | is-empty) {
            error make { msg: $"rebase-patches: no fork package named ($name); known: (($forks | get name) | str join ', ')" }
          }
          $hit
        } else {
          $forks
        }

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
