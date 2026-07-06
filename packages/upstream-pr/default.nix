# `nix run .#upstream-pr -- <pkg> <patch> [--open] [--dry-run]`: contribute ONE
# of our fork patches upstream without carrying the rest of the series.
#
# We keep a de-forked patch series (packages/<pkg>/patches, see
# lib/util/patched-src.nix) pinned at an OLDER upstream base. To send a single
# patch upstream, we cannot just push our whole branch: it drags in every other
# patch and is based on a stale rev. So this tool:
#
#   1. Reads the patch's ancestor closure from dag.json (the derived dependency
#      graph). A truly independent patch contributes just itself; a patch with
#      real deps drags its closure, and we warn listing the extra patches so the
#      author knows the upstream PR is not single-commit.
#   2. Fetches the upstream repo's DEFAULT branch tip (not our pinned base), so
#      the contribution targets current upstream.
#   3. `git am --3way` the closure onto that tip. The 3-way merge absorbs
#      mechanical drift between our old base and the upstream tip; a real
#      collision fails loudly (this is exactly where old-base-vs-tip drift
#      surfaces, and a human must rebase the patch).
#   4. Pushes the branch to an indexable-inc fork of the upstream repo (created
#      with `gh repo fork --clone=false` if absent). Pushing to OUR fork is
#      fine; it is not the outward act.
#   5. Prints the ready-to-open compare URL. With `--open`, additionally opens a
#      DRAFT PR upstream. Default is prepare-only: opening the upstream PR is the
#      outward act and stays behind an explicit `--open` a human invokes.
#
# `--dry-run` runs the whole flow (closure, fetch, am, branch) but skips the
# push and PR, printing what it WOULD push. Used to validate content without
# touching any remote.
#
# The fork-package mapping (upstream URL, patch dir) is data from
# lib/fork-packages.nix; the dependency closure is data from each series'
# dag.json. Both are read, not hardcoded. A downstream repo (e.g. ix) reuses this
# one tool for its own forks via `--mapping <its-fork.json>` (run from its repo
# root); the baked-in list is index's default. One tool, parameterized by data.
{
  ix,
  formats,
  writeNushellApplication,
  git,
  gh,
  coreutils,
}: let
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  # Reuse the DAG closure logic from the one owner of that code (rebase-patches),
  # referenced through the package registry root rather than a `../` literal.
  dagLib = ix.paths.packagesRoot + "/rebase-patches/dag-lib.nu";
in
  writeNushellApplication {
    name = "upstream-pr";
    meta = {
      description = "Contribute one fork patch upstream (its dag.json ancestor closure) via a fork branch + compare URL";
      mainProgram = "upstream-pr";
    };
    runtimeInputs = [
      git
      gh
      coreutils
    ];
    text = ''
      # nu
      # Run from the repo root: `nix run .#upstream-pr -- <pkg> <patch> [--open] [--dry-run]`.
      use ${dagLib} *

      const fork_data = "${forkData}"
      const org = "indexable-inc"

      # The fork-package mapping to drive: the caller-supplied `--mapping` path (a
      # downstream repo pointing this one tool at its own fork list) else index's
      # baked-in list. One tool, parameterized by data, never copied.
      def "mapping path" [override?: string]: nothing -> string {
        if $override == null { $fork_data } else { $override }
      }

      # Resolve a fork record by name against `mapping`, erroring with the known set.
      def "fork by-name" [name: string, mapping?: string]: nothing -> record {
        let forks = (open (mapping path $mapping))
        let hit = ($forks | where name == $name)
        if ($hit | is-empty) {
          error make { msg: $"upstream-pr: no fork package named ($name); known: (($forks | get name) | str join ', ')" }
        }
        $hit | first
      }

      # Owner/repo slug from an upstream https git URL, e.g.
      # https://github.com/openai/codex.git -> {owner: openai, repo: codex}.
      def "url slug" [url: string]: nothing -> record {
        let trimmed = ($url | str replace --regex '\.git$' "" | str replace --regex '/$' "")
        let parts = ($trimmed | split row "/")
        {owner: ($parts | get ($parts | length | $in - 2)), repo: ($parts | last)}
      }

      # A filesystem/branch-safe slug from a patch file name: drop the NNNN- prefix
      # and the .patch suffix, keep the descriptive middle.
      def "patch slug" [patch: string]: nothing -> string {
        $patch
        | str replace --regex '^[0-9]+-' ""
        | str replace --regex '\.patch$' ""
        | str downcase
        | str replace --all --regex '[^a-z0-9]+' "-"
        | str trim --char "-"
      }

      def main [
        pkg: string    # fork package name (codex | btop | clippy)
        patch: string  # patch file name (or its NNNN prefix / unique substring)
        --open         # also open a DRAFT PR upstream (outward act; default: prepare only)
        --dry-run      # run the whole flow but skip push + PR (validate content)
        --mapping: string # fork-package JSON to drive (default: index's baked-in list)
      ] {
        let fork = (fork by-name $pkg $mapping)
        let patch_dir = ($fork.patchDir | path expand)
        let dag_file = ($patch_dir | path join "dag.json")
        if not ($dag_file | path exists) {
          error make { msg: $"upstream-pr: ($pkg): missing dag.json in ($fork.patchDir); run `nix run .#rebase-patches -- dag`" }
        }
        let doc = (open --raw $dag_file | from json)
        let all_patches = ($doc.nodes | get patch)

        # Resolve the requested patch to an exact node name (exact, then prefix,
        # then unique substring).
        let target = (resolve patch $patch $all_patches)
        print $"(ansi cyan)upstream-pr: ($pkg): target patch ($target)(ansi reset)"

        # Ancestor closure from the DAG, in NNNN order, plus the target last.
        let deps_of = ($doc.nodes | reduce --fold {} {|nd, acc| $acc | insert $nd.patch $nd.deps })
        let closure = (dag closure $deps_of $target)
        let pos = ($all_patches | enumerate | reduce --fold {} {|it, acc| $acc | insert $it.item $it.index })
        let ordered = (($closure | append $target) | uniq | sort-by {|p| $pos | get $p })
        if ($closure | is-not-empty) {
          print $"(ansi yellow)upstream-pr: ($pkg): ($target) is NOT independent; its upstream contribution drags (($closure | length)) ancestor patch\(es\):(ansi reset)"
          for c in ($closure | sort-by {|p| $pos | get $p }) { print $"  - ($c)" }
          print $"(ansi yellow)upstream-pr: consider splitting, or send the closure as one PR.(ansi reset)"
        } else {
          print $"upstream-pr: ($pkg): ($target) is independent; contributing it alone."
        }

        let slug = (url slug $fork.url)
        let branch = $"upstream-pr/($pkg)/(patch slug $target)"

        # Scratch repo: fetch the upstream DEFAULT branch tip and `git am` the
        # closure onto it with 3-way. Deterministic config so a developer's
        # global git settings do not perturb the apply.
        let scratch = (mktemp --directory --tmpdir $"upstream-pr-($pkg).XXXXXX")
        git -C $scratch init --quiet
        dag neutralize-config $scratch
        print $"upstream-pr: fetching (($slug.owner)/($slug.repo)) default branch tip..."
        git -C $scratch remote add upstream $fork.url
        # Discover the default branch (HEAD) of upstream, then fetch just it.
        let head_ref = (
          git -C $scratch ls-remote --symref upstream HEAD
          | lines
          | where {|l| $l | str starts-with "ref:" }
          | first
          | parse --regex 'ref:\s+refs/heads/(?<b>\S+)\s+HEAD'
          | get b.0
        )
        print $"upstream-pr: upstream default branch is ($head_ref)"
        git -C $scratch fetch --quiet upstream $head_ref
        let tip = (git -C $scratch rev-parse FETCH_HEAD | str trim)
        git -C $scratch checkout --quiet -b $branch $tip

        # Apply the closure onto the tip with 3-way. On conflict, fail loudly:
        # this is where our old base drifting from the upstream tip shows up.
        let patch_files = ($ordered | each {|p| $patch_dir | path join $p })
        let am = (do { git -C $scratch am --3way ...$patch_files } | complete)
        if $am.exit_code != 0 {
          let unmerged = (git -C $scratch diff --name-only --diff-filter=U | lines)
          # `git am --3way` can fail with no unmerged entries when a patch adds a
          # file that already exists upstream, or a hunk has no 3-way base. Fall
          # back to git's own message so the failure is legible either way.
          let detail = if ($unmerged | is-not-empty) {
            $"conflicting files: [($unmerged | str join ', ')]"
          } else {
            $"git am output:\n(($am.stdout + $am.stderr) | lines | last 12 | str join (char newline))"
          }
          git -C $scratch am --abort
          error make { msg: $"upstream-pr: ($pkg): `git am --3way` of the closure did not apply onto the upstream tip ($tip). The patch needs rebasing against current upstream before it can be contributed \(old-base-vs-tip drift\). ($detail). Scratch repo: ($scratch)" }
        }
        let n_commits = (git -C $scratch rev-list --count $"($tip)..HEAD" | str trim)
        print $"(ansi green)upstream-pr: ($pkg): applied ($n_commits) commit\(s\) cleanly onto ($slug.owner)/($slug.repo)@($head_ref) (($tip | str substring 0..9))(ansi reset)"

        if $dry_run {
          print $"(ansi green)upstream-pr: --dry-run: would push branch ($branch) to ($org)/($slug.repo) and print a compare URL. Commits:(ansi reset)"
          git -C $scratch log --oneline $"($tip)..HEAD"
          print $"upstream-pr: scratch repo left for inspection: ($scratch)"
          return
        }

        # Ensure an indexable-inc fork of the upstream exists, then push.
        ensure fork $slug
        print $"upstream-pr: pushing ($branch) to ($org)/($slug.repo)..."
        git -C $scratch remote add fork $"https://github.com/($org)/($slug.repo).git"
        git -C $scratch push --force fork $branch

        let compare = $"https://github.com/($slug.owner)/($slug.repo)/compare/($head_ref)...($org):($slug.repo):($branch)?expand=1"
        print $"(ansi green)upstream-pr: ($pkg): pushed. Ready-to-open compare URL:(ansi reset)"
        print $"  ($compare)"

        if $open {
          # The outward act, gated behind --open. Draft only.
          let title = (git -C $scratch log -1 --format='%s' HEAD | str trim)
          print $"(ansi yellow)upstream-pr: opening DRAFT PR upstream ($slug.owner)/($slug.repo) <- ($org):($branch)...(ansi reset)"
          (
            gh pr create
              --repo $"($slug.owner)/($slug.repo)"
              --base $head_ref
              --head $"($org):($branch)"
              --title $title
              --draft
              --body $"Contributed from indexable-inc's in-repo patch series. Single-patch closure of `($target)`.\n\nAuto-prepared by `nix run .#upstream-pr`."
          )
        } else {
          print $"upstream-pr: prepare-only. Re-run with `--open` to open a DRAFT PR upstream, or open the compare URL by hand."
        }

        rm --recursive --force $scratch
      }

      # Ensure `indexable-inc/<repo>` exists as a fork of the upstream; create it
      # (non-cloning) if absent. Idempotent.
      def "ensure fork" [slug: record] {
        let exists = (do { gh repo view $"($org)/($slug.repo)" } | complete)
        if $exists.exit_code == 0 { return }
        print $"upstream-pr: forking ($slug.owner)/($slug.repo) into ($org) (one-time)..."
        gh repo fork $"($slug.owner)/($slug.repo)" --org $org --clone=false
      }

      # Resolve a user-provided patch reference to an exact node name: exact match,
      # else unique NNNN-prefix, else unique substring.
      def "resolve patch" [ref: string, names: list<string>] {
        if $ref in $names { return $ref }
        let by_prefix = ($names | where {|n| $n | str starts-with $ref })
        if ($by_prefix | length) == 1 { return ($by_prefix | first) }
        let by_sub = ($names | where {|n| $n | str contains $ref })
        if ($by_sub | length) == 1 { return ($by_sub | first) }
        let candidates = (($by_prefix | append $by_sub) | uniq)
        if ($candidates | is-empty) {
          error make { msg: $"upstream-pr: no patch matching '($ref)'. Known: (($names) | str join ', ')" }
        }
        error make { msg: $"upstream-pr: '($ref)' is ambiguous; matches: (($candidates) | str join ', ')" }
      }
    '';
  }
