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
#   4. Runs the fork's `preflight` commands (lib/fork-packages.nix) in the
#      patched scratch checkout: the target repo's own cheap pre-submit gates
#      (fmt-level, mirroring the first steps of its CI). A red preflight aborts
#      the contribution loudly BEFORE anything is pushed; an upstream PR that
#      fails `cargo fmt` in its first CI step reads as low-effort to
#      maintainers (nushell/nushell#18549).
#   5. Pushes the branch to an indexable-inc fork of the upstream repo (created
#      with `gh repo fork --clone=false` if absent). Pushing to OUR fork is
#      fine; it is not the outward act.
#   6. Prints the ready-to-open compare URL. With `--open`, additionally opens
#      the PR upstream READY FOR REVIEW (pass `--draft` to open a draft
#      instead). Ready is the default because the preflight and template
#      rendering above are exactly the pre-submit bar; a PR parked as a draft
#      signals not-ready and sits unreviewed. Default is prepare-only: opening
#      the upstream PR is the outward act and stays behind an explicit `--open`
#      a human invokes.
#
# The PR's title and body come FROM THE PATCH ITSELF: subject = title, commit
# message body = PR body (one fact, one home; the fork mapping deliberately has
# no duplicate description field), plus AI attribution and a link back to the
# patch file of record. When the target repo ships a PR template
# (.github/pull_request_template.md and the standard fallback locations), the
# body is RENDERED INTO the template's `## ` sections instead: Description <-
# the commit body, a release-notes section <- the patch's `releaseNotes` from
# the mapping, an additional-notes section <- `prExtra` + the attribution
# block. A template section this tool cannot fill refuses loudly rather than
# opening a noncompliant PR; a repo with no template keeps the plain
# composition (body + `prExtra` + attribution). A body-less commit is
# refused; the `patch-dag-<name>` check enforces the same for every
# attempt-marked patch so the failure happens in CI, not mid-contribution.
#
# `--dry-run` runs the whole flow (closure, fetch, am, branch, preflight) but
# skips the push and PR, printing what it WOULD push. Used to validate content
# without touching any remote.
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
  # Preflight commands are arbitrary shell strings from the fork mapping,
  # run with `bash -ec` in the scratch checkout.
  bash,
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
      bash
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

      # The https blob URL of the patch file of record in the INVOKING repo (so a
      # downstream mapping links to its own repo), derived from the `origin`
      # remote in either ssh or https form. Returns null with a loud note when
      # origin is absent or not a github URL: the PR body then omits the link
      # rather than fabricating one.
      def "origin blob-link" [patch_dir: string, patch: string]: nothing -> any {
        let res = (do { git remote get-url origin } | complete)
        if $res.exit_code != 0 {
          print $"(ansi yellow)upstream-pr: no `origin` remote here; the PR body will omit the patch-of-record link.(ansi reset)"
          return null
        }
        let url = ($res.stdout | str trim)
        let m = ($url | parse --regex 'github\.com[:/](?<owner>[^/]+)/(?<repo>[^/]+?)(\.git)?$')
        if ($m | is-empty) {
          print $"(ansi yellow)upstream-pr: origin ($url) is not a parseable github URL; the PR body will omit the patch-of-record link.(ansi reset)"
          return null
        }
        let o = ($m | first)
        $"https://github.com/($o.owner)/($o.repo)/blob/main/($patch_dir)/($patch)"
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

      # The target repo's PR template file in the scratch checkout, or null.
      # GitHub resolves templates from .github/, the repo root, and docs/, in
      # either case; the scratch repo has the upstream default-branch tip
      # checked out, so the working tree is the source of truth.
      def "template find" [scratch: string]: nothing -> any {
        let hits = (
          [
            ".github/pull_request_template.md"
            ".github/PULL_REQUEST_TEMPLATE.md"
            "pull_request_template.md"
            "PULL_REQUEST_TEMPLATE.md"
            "docs/pull_request_template.md"
            "docs/PULL_REQUEST_TEMPLATE.md"
          ]
          | where {|c| $scratch | path join $c | path exists }
        )
        if ($hits | is-empty) { null } else { $scratch | path join ($hits | first) }
      }

      # Render the PR body INTO the target repo's PR template: each `## `
      # section is filled from the source that owns it (Description <- the
      # commit body, release notes <- the patch's `releaseNotes` from the fork
      # mapping, additional notes <- `prExtra` + the attribution block). A
      # section this mapping does not recognize, or a release-notes section
      # for a patch that declares no `releaseNotes`, REFUSES loudly: a
      # template-noncompliant PR reads as low-effort to the maintainers
      # receiving it (nushell/nushell#18549), so there is no silent fallback.
      def "template render" [
        template: string     # raw template text from the upstream checkout
        pkg: string          # fork name, for error messages
        target: string       # patch file name, for error messages
        commit_body: string  # fills the Description section
        release_notes: any   # `patches.<patch>.releaseNotes`, or null
        notes: string        # prExtra + attribution, fills additional notes
      ]: nothing -> string {
        let headings = (
          $template
          | lines
          | where {|l| $l | str starts-with "## " }
          | each {|l| $l | str replace --regex '^##\s+' "" | str trim }
        )
        if ($headings | is-empty) {
          error make { msg: $"upstream-pr: ($pkg): the target repo has a PR template with no `## ` sections; this tool cannot render into it. Open the PR by hand, following the template." }
        }
        $headings
        | each {|h|
          let hl = ($h | str downcase)
          let content = (
            if ($hl | str contains "description") {
              $commit_body
            } else if (($hl | str contains "user-facing") or ($hl | str contains "release note")) {
              if $release_notes == null {
                error make { msg: $"upstream-pr: ($pkg): the target repo's PR template requires a '($h)' section, but ($target) declares no `releaseNotes` in the fork mapping \(lib/fork-packages.nix\). Write the user-facing change in release-note style \(or 'n/a'\) there; NOT opening a template-noncompliant PR." }
              }
              $release_notes | str trim
            } else if (($hl | str contains "additional note") or ($hl == "notes")) {
              $notes
            } else {
              error make { msg: $"upstream-pr: ($pkg): the target repo's PR template has a '($h)' section this tool does not know how to fill. Extend the section mapping in packages/upstream-pr or open the PR by hand; NOT opening a template-noncompliant PR." }
            }
          )
          $"## ($h)\n\n($content)"
        }
        | str join "\n\n"
      }

      def main [
        pkg: string    # fork package name (codex | btop | clippy)
        patch: string  # patch file name (or its NNNN prefix / unique substring)
        --open         # also open the PR upstream (outward act; default: prepare only)
        --draft        # with --open: open the PR as a draft (default: ready for review)
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

        # Per-repo preflight (`preflight` in the fork mapping): the target
        # repo's own cheap pre-submit gates (fmt-level checks mirroring the
        # first steps of its CI, never full test suites), run in the patched
        # scratch checkout so the EXACT tree we would push passes them. A red
        # preflight aborts the contribution loudly before anything is pushed:
        # nushell/nushell#18549 shipped a `cargo fmt` failure that turned the
        # whole upstream CI matrix red in seconds. Commands run via `bash -ec`
        # with the invoking environment's toolchain; a missing tool fails the
        # same way (loudly), never skips.
        let preflight = ($fork | get -o preflight | default [])
        for cmd in $preflight {
          print $"upstream-pr: ($pkg): preflight: ($cmd)"
          let res = (do { cd $scratch; bash -ec $cmd } | complete)
          if $res.exit_code != 0 {
            print ($res.stdout)
            print ($res.stderr)
            error make { msg: $"upstream-pr: ($pkg): preflight `($cmd)` FAILED in the patched checkout; the upstream PR would open with red CI. Fix the patch series first. Scratch repo: ($scratch)" }
          }
          print $"(ansi green)upstream-pr: ($pkg): preflight `($cmd)` passed(ansi reset)"
        }

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
          # The outward act, gated behind --open. Title and body come from the
          # patch's own commit message (one fact, one home: nix carries no
          # duplicate description field), so a body-less commit is refused
          # loudly; the `patch-dag-<name>` check enforces the same for
          # attempt-marked patches before it ever gets here.
          let title = (git -C $scratch log -1 --format='%s' HEAD | str trim)
          let commit_body = (git -C $scratch log -1 --format='%b' HEAD | str trim)
          if ($commit_body | is-empty) {
            error make { msg: $"upstream-pr: ($pkg): ($target) has no commit-message body; write the why in the commit body \(it becomes the upstream PR description\)." }
          }
          # Optional upstream-specific PR content that does not belong in a
          # commit message: `prExtra` (issue refs, checklists) and
          # `releaseNotes` (user-facing release-note text for templates that
          # require it), declared per patch in the fork mapping.
          let patch_meta = ($fork | get -o patches | default {} | get -o $target | default {})
          let pr_extra = ($patch_meta | get -o prExtra)
          let release_notes = ($patch_meta | get -o releaseNotes)
          # Link back to the patch file of record in OUR repo, derived from the
          # invoking repo's origin remote so a downstream mapping links to its
          # own repo. Best-effort but loud: no parseable origin means no link.
          let patch_link = (origin blob-link $fork.patchDir $target)
          let attribution = (
            [
              "---"
              (if $patch_link != null {
                $"Contributed from a maintained fork patch series; the patch of record is ($patch_link)."
              } else {
                $"Contributed from a maintained fork patch series \(patch ($target)\)."
              })
              "Prepared with AI assistance (Claude); directed and reviewed by a human maintainer."
            ] | str join "\n\n"
          )
          # prExtra + attribution together are the "anything else reviewers
          # should know" content: under the template's additional-notes
          # section when the repo has a template, appended after the body
          # otherwise.
          let notes = (
            (if $pr_extra != null { [$pr_extra] } else { [] })
            | append [$attribution]
            | str join "\n\n"
          )
          # Follow the target repo's conventions: render into its PR template
          # when it ships one (refusing loudly on any section we cannot
          # fill); keep the plain composition when it does not.
          let template_path = (template find $scratch)
          let body = (
            if $template_path != null {
              print $"upstream-pr: ($pkg): rendering the PR body into the target repo's template \(($template_path | path relative-to $scratch)\)"
              template render (open --raw $template_path) $pkg $target $commit_body $release_notes $notes
            } else {
              [$commit_body $notes] | str join "\n\n"
            }
          )
          let kind = (if $draft { "DRAFT" } else { "ready-for-review" })
          print $"(ansi yellow)upstream-pr: opening ($kind) PR upstream ($slug.owner)/($slug.repo) <- ($org):($branch)...(ansi reset)"
          (
            gh pr create
              --repo $"($slug.owner)/($slug.repo)"
              --base $head_ref
              --head $"($org):($branch)"
              --title $title
              --body $body
              ...(if $draft { ["--draft"] } else { [] })
          )
        } else {
          print $"upstream-pr: prepare-only. Re-run with `--open` to open the PR upstream \(add `--draft` for a draft\), or open the compare URL by hand."
        }

        rm --recursive --force $scratch
      }

      # Ensure `indexable-inc/<repo>` exists as a fork of the upstream; create it
      # (non-cloning) if absent. Idempotent.
      def "ensure fork" [slug: record] {
        let exists = (do { gh repo view $"($org)/($slug.repo)" } | complete)
        if $exists.exit_code == 0 { return }
        print $"upstream-pr: forking ($slug.owner)/($slug.repo) into ($org) once..."
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
