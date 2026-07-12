# `nix run .#upstream-sync [-- <pkg> [<patch>]] [--open] [--dry-run] [--check-stale]`:
# drive the de-fork UPSTREAMING loop. This is the layer above `upstream-pr` (the
# per-patch branch/am/push/PR mechanism) and `rebase-patches` (the base-bump
# regenerator): it decides which patches to act on from the hand-written
# declarative intent, tracks the live state of the PRs we open, spots duplicate
# upstream PRs, and retires patches that land upstream.
#
#
# `nix run .#upstream-sync -- drift [--json|--markdown] [name]` is the read-only
# companion report (RFC 0010, #2098): per fork, how far the pinned base trails
# upstream's default branch (commits behind + base age), the declared patch
# stances, how many tracked patches are retired-awaiting-drop, and a one-word
# next action. The fork-sync cron surfaces it in its step summary and rolling
# PR body.
#
# The two-sided design the user set:
#   - DECLARATIVE INTENT lives in nix (lib/fork-packages.nix), hand-written: each
#     patch's `upstream = attempt|hold|never` + one-line reason, and a per-repo
#     `upstreamPolicy` (prsWelcome / aiPrsAllowed / citation / notes). `attempt`
#     is the human gate that authorizes the outward act; the tool opens a real
#     upstream PR ONLY for a patch explicitly marked `attempt`.
#   - LIVE STATE is GENERATED, never hand-written: `upstream-status.json` sits
#     next to each series' `dag.json` (the mapping's natural per-repo owner, so
#     live state is colocated with the patches and the DAG it tracks). It is
#     tool-owned and committed like a lock file: per patch the tracked PR url +
#     state + last-checked, detected duplicate upstream PRs, and retirement
#     status, plus an append-only human-readable transition `log`.
#
# The walk is by UNIT: a `prSeries` intent group (a multi-patch feature series,
# e.g. nix's build-status-dir set) is one unit and one upstream PR carrying every
# member; the DAG's textual independence deliberately does not split a semantic
# series into per-commit PRs. Everything else is a single-patch unit. Per unit of
# each selected fork:
#   1. If we already track a PR: refresh its state via `gh pr view` (open / draft
#      / merged / closed) FIRST, regardless of intent or repo blocks, because a
#      refresh is read-only and intent gates only NEW outward acts (a patch
#      demoted to hold/never after its PR opened must keep tracking it). If
#      merged, mark every member `retired = true` and record it: the NEXT base
#      bump's `rebase-patches` run should drop the patch (it becomes an empty
#      cherry against the new base), and this tool wires a retirement note into
#      the plan so a human/agent verifies the drop.
#   2. Else (attempt units on non-blocked repos) search the upstream repo for a
#      DUPLICATE/related PR by the unit head's title keywords. If found, RECORD
#      it and SKIP loudly (a human or agent can comment on the existing PR
#      instead of opening a competing one).
#   3. Else, if `--open` was passed, open the PR by delegating to
#      `upstream-pr --open` (its DAG-closure/am/push/draft-PR mechanism, one
#      owner), passing every member and `--message-from` the series head. For
#      forks opted into the closure build gates (`closureGates = true` in
#      lib/fork-packages.nix; RFC 0010 A3), every member's gate derivation
#      (`forkClosureGates.<system>.<fork>.<patch>`) is built FIRST and a red
#      gate aborts the unit's PR-opening: `upstream-pr` ships the unit as its
#      dag.json closure against the bare base, so a non-building closure means
#      the upstream PR would be broken. Only the `--open` path pays this build;
#      refresh/search/would-open never build anything.
#      The PR title/body come from the head's commit message plus AI attribution
#      and a link back to our patch file. Opening a PR is the outward act, DOUBLY
#      gated: the patch must be marked `attempt` in nix (intent gate) AND
#      `--open` must be passed (invocation gate). Without `--open`, the safe
#      default, the tool still refreshes/searches/retires and writes the status
#      file, and reports which patches WOULD open. `--dry-run` suppresses the
#      status write too and is mutually exclusive with `--open`.
#
# Repos where PRs are unwelcome (`prsWelcome = false`) or AI PRs are banned
# (`aiPrsAllowed = "false"`) are skipped at the repo level: the tool refuses to
# open any PR there regardless of a per-patch `attempt`, so a banned repo cannot
# leak a PR.
#
# DAG FORMAT (dag.json) STAYS JSON, NOT NIX. dag.json is machine-DERIVED:
# `rebase-patches` regenerates it from apply-tests and the `patch-dag-<name>`
# check staleness-gates it. Moving it to hand-editable nix would invite drift
# between the declared graph and the real apply behavior, and buy nothing: nix
# reads JSON natively (`lib.importJSON`) and this tool reads it as plain data via
# nushell. Declarative INTENT (what we WANT) belongs in nix; DERIVED state (the
# real dependency graph, and the live PR state this tool owns) belongs in
# generated JSON. Same split as `upstream-status.json` here.
#
# The fork mapping (URL, patch dir, intent, policy) is data from
# lib/fork-packages.nix; the DAG closure is data from dag.json; both are read,
# not hardcoded. A downstream repo (ix) reuses this one tool for its own forks
# via `--mapping <its-fork.json>` run from its repo root.
{
  ix,
  formats,
  writeNushellApplication,
  git,
  gh,
  coreutils,
  # Pinned client for the closure-gate preflight `nix build` (same posture as
  # lib/fork-updater.nix's updateScript).
  nix,
  # Sibling repo packages, threaded under one name (see lib/packages.nix); we take
  # the PR mechanism (`upstream-pr`) from here rather than a bare callPackage arg,
  # which the package set does not expose flat.
  repoPackages,
  runCommand,
  nushell,
}: let
  inherit (repoPackages) upstream-pr;
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  # Shared patch-selector + subject helpers from their one owner (rebase-patches'
  # dag-lib), referenced through the package registry root rather than a `../`
  # literal, so this tool and upstream-pr cannot diverge on selector semantics.
  dagLib = ix.paths.packagesRoot + "/rebase-patches/dag-lib.nu";
  package = writeNushellApplication {
    name = "upstream-sync";
    meta = {
      description = "Drive the de-fork upstreaming loop: track PR state, find duplicates, retire merged patches, and open PRs for attempt-marked patches";
      mainProgram = "upstream-sync";
    };
    runtimeInputs = [
      git
      gh
      coreutils
      nix
      upstream-pr
    ];
    text = ''
      # nu
      # Run from the repo root: `nix run .#upstream-sync [-- <pkg> [<patch>]]`.
      use ${dagLib} *

      const fork_data = "${forkData}"

      # --- mapping + resolution (shared idioms with upstream-pr) ----------------

      # The fork mapping to drive: the caller `--mapping` path (a downstream repo
      # pointing this one tool at its own list) else index's baked-in list.
      def "mapping path" [override?: string]: nothing -> string {
        if $override == null { $fork_data } else { $override }
      }

      # Resolve the selected fork records from an optional name against `mapping`.
      def "fork select" [name?: string, mapping?: string]: nothing -> list<record> {
        let forks = (open (mapping path $mapping))
        if $name == null { return $forks }
        let hit = ($forks | where name == $name)
        if ($hit | is-empty) {
          error make { msg: $"upstream-sync: no fork package named ($name); known: (($forks | get name) | str join ', ')" }
        }
        $hit
      }

      # Owner/repo slug from an upstream https git URL.
      def "url slug" [url: string]: nothing -> record {
        let trimmed = ($url | str replace --regex '\.git$' "" | str replace --regex '/$' "")
        let parts = ($trimmed | split row "/")
        {owner: ($parts | get ($parts | length | $in - 2)), repo: ($parts | last)}
      }

      # Is this upstream a GitHub repo? The gh-based PR + search path only works
      # for github.com; a non-github host (e.g. mesa on gitlab.freedesktop.org) has
      # no gh path, so we cannot track or open there.
      def "is github" [url: string]: nothing -> bool {
        $url | str contains "github.com"
      }

      # --- status file (tool-owned, committed like a lock file) -----------------

      # Path to a fork's upstream-status.json: next to its dag.json in the patch
      # dir, the mapping's natural per-repo owner for live state.
      def "status path" [fork: record]: nothing -> string {
        $fork.patchDir | path expand | path join "upstream-status.json"
      }

      # Load a fork's status doc, or an empty skeleton if none exists yet. The
      # `patches` map is keyed by patch file name; `log` is append-only.
      def "status load" [fork: record]: nothing -> record {
        let p = (status path $fork)
        if ($p | path exists) {
          open --raw $p | from json
        } else {
          {
            comment: "Tool-owned live upstreaming state; generated by `nix run .#upstream-sync`, committed like a lock file. Do not hand-edit. Declarative intent (attempt/hold/never) lives in lib/fork-packages.nix."
            lastChecked: null
            patches: {}
            log: []
          }
        }
      }

      # Serialize the status doc to the exact bytes we commit: pretty JSON, two-space
      # indent, trailing newline (matches dag.json's format owner).
      def "status to-json" [doc: record]: nothing -> string {
        ($doc | to json --indent 2) + "\n"
      }

      # Save a fork's status doc unless dry-run, in which case just report the diff
      # intent. Writing is idempotent: re-running with no upstream change reproduces
      # the same bytes (only lastChecked + any new transitions move).
      def "status save" [fork: record, doc: record, dry_run: bool] {
        let p = (status path $fork)
        if $dry_run {
          print $"(ansi cyan)upstream-sync: --dry-run: would write ($fork.name) status to ($p)(ansi reset)"
        } else {
          (status to-json $doc) | save --force --raw $p
          print $"(ansi green)upstream-sync: ($fork.name): wrote ($p)(ansi reset)"
        }
      }

      # Append a timestamped transition line to the status log (human-readable).
      def "log append" [doc: record, line: string]: nothing -> record {
        let stamp = (date now | format date "%Y-%m-%dT%H:%M:%SZ")
        print $"  (ansi yellow)• ($line)(ansi reset)"
        $doc | update log { append $"($stamp) ($line)" }
      }

      # --- PR state via gh ------------------------------------------------------

      # Refresh a tracked PR's live state. Returns a record {url, number, state,
      # checkedAt} with state one of open|draft|merged|closed, or null if the PR
      # can no longer be read (deleted/renamed). `state` collapses gh's separate
      # `state` (OPEN/CLOSED/MERGED) and `isDraft` into one field.
      def "pr refresh" [slug: record, number: int]: nothing -> any {
        let res = (
          do {
            gh pr view $number --repo $"($slug.owner)/($slug.repo)" --json state,isDraft,url,number
          } | complete
        )
        if $res.exit_code != 0 { return null }
        let j = ($res.stdout | from json)
        let state = (
          if $j.state == "MERGED" { "merged" }
          else if $j.state == "CLOSED" { "closed" }
          else if $j.isDraft { "draft" }
          else { "open" }
        )
        {url: $j.url, number: $j.number, state: $state, checkedAt: (date now | format date "%Y-%m-%dT%H:%M:%SZ")}
      }

      # Distinctive lowercase tokens of a patch subject: alphanumerics, min length
      # 4, minus generic contribution/domain filler that would match everything.
      # Used to build a tight duplicate query and to post-filter gh's fuzzy hits.
      def "subject tokens" [subject: string]: nothing -> list<string> {
        let stop = [
          "add" "fix" "the" "and" "for" "with" "from" "into" "when" "test" "tests"
          "doc" "docs" "note" "feature" "command" "support" "allow" "make" "use"
          "libstore" "libutil" "libexpr" "nix" "build" "status"
        ]
        $subject
        | str downcase
        | split row --regex '[^a-z0-9]+'
        | where {|t| ($t | str length) >= 4 }
        | where {|t| $t not-in $stop }
        | uniq
      }

      # Search the upstream repo for OPEN PRs that plausibly DUPLICATE this patch,
      # to record and skip rather than open a competing one. GitHub's issue/PR
      # search treats a bare space as AND, which would miss a duplicate that
      # rewords or drops one token, so the query ORs the distinctive title tokens
      # (capped at 6: GitHub allows at most five boolean operators per query) for
      # broad recall, and a MAJORITY title-overlap post-filter (>= 60% of our
      # tokens, floor 2) restores precision. A flat 2-token floor was measurably
      # too loose under OR recall: "don't crash the daemon when a GC roots
      # client" matched an unrelated client-settings PR on {daemon, client}, and
      # "inaccessible default lookup-path entries" matched a docs-only PR on
      # {lookup, path}. Real duplicates of the same change share most distinctive
      # words. The tension is deliberate: a missed dup just costs a human dedupe,
      # while a false dup silently BLOCKS an attempt, so precision wins ties.
      # Best-effort: any failure or a tokenless subject returns [] so the loop
      # never stalls.
      def "pr find-duplicates" [slug: record, subject: string]: nothing -> list<record> {
        let tokens = (subject tokens $subject)
        if ($tokens | is-empty) { return [] }
        let query = $"(($tokens | first 6 | str join ' OR ')) in:title"
        let res = (
          do {
            gh search prs $query --repo $"($slug.owner)/($slug.repo)" --state open --limit 20 --json url,number,title
          } | complete
        )
        if $res.exit_code != 0 { return [] }
        # ceil(0.6 * n) via integer math, floored at 2.
        let need = ([2, ((($tokens | length) * 3 + 4) // 5)] | math max)
        ($res.stdout | from json)
        | where {|hit|
          let ht = (subject tokens $hit.title)
          (($tokens | where {|t| $t in $ht }) | length) >= $need
        }
      }

      # --- drift report (read-only; RFC 0010, #2098) ----------------------------

      # The pinned base rev of a fork's input from the committed flake.lock in
      # the CWD (the tool runs from the repo root; a downstream --mapping repo
      # reads its own lock the same way). Null when the lock or input is absent.
      def "lock rev" [input: string]: nothing -> any {
        if not ("flake.lock" | path exists) { return null }
        let node = (open --raw flake.lock | from json | get -o nodes | default {} | get -o $input)
        if $node == null { return null }
        $node | get -o locked.rev
      }

      # Is this upstream on a GitLab host (e.g. mesa on gitlab.freedesktop.org)?
      def "is gitlab" [url: string]: nothing -> bool {
        $url | str contains "gitlab."
      }

      # A gh api read that DEGRADES instead of failing: a forge error becomes a
      # stderr warning and a null cell, so one unreachable forge cannot take the
      # whole drift table down.
      def "gh read" [ctx: string, path: string, jq: string]: nothing -> any {
        let res = (do { gh api $path --jq $jq } | complete)
        if $res.exit_code != 0 {
          print -e $"(ansi yellow)upstream-sync: drift: ($ctx): gh api ($path) failed; cell left unknown(ansi reset)"
          return null
        }
        $res.stdout | str trim
      }

      # Base-commit committer date on a GitLab host, or null. GitLab drift is
      # DELIBERATELY base-age-only: the compare API enumerates every commit in
      # the range (thousands on a months-old mesa pin), so the cheap single-commit
      # lookup is the reliable unauthenticated read and commits-behind stays
      # unknown (the RFC allows exactly this degradation).
      def "gitlab base-date" [url: string, rev: string]: nothing -> any {
        let u = ($url | url parse)
        let project = ($u.path | str trim --left --char "/" | str replace --regex '\.git$' "" | url encode --all)
        let endpoint = $"https://($u.host)/api/v4/projects/($project)/repository/commits/($rev)"
        try {
          http get $endpoint | get committed_date
        } catch {
          print -e $"(ansi yellow)upstream-sync: drift: ($endpoint) unreachable; base age left unknown(ansi reset)"
          null
        }
      }

      # One fork's drift facts. A null cell means "unknown" (forge unreachable or
      # input not in flake.lock), never a crash: the report must survive a broken
      # forge and still render the other rows.
      def "drift row" [fork: record]: nothing -> record {
        let slug = (url slug $fork.url)
        let rev = (lock rev $fork.input)
        if $rev == null {
          print -e $"(ansi yellow)upstream-sync: drift: ($fork.name): input ($fork.input) has no locked rev in flake.lock(ansi reset)"
        }
        let forge = (
          if (is github $fork.url) { "github" }
          else if (is gitlab $fork.url) { "gitlab" }
          else { "other" }
        )

        # Commits behind = ahead_by of `pinned...default_branch`: how many
        # commits upstream's default branch has that our pinned base does not.
        let behind = (
          if $forge != "github" or $rev == null { null } else {
            let branch = (gh read $fork.name $"repos/($slug.owner)/($slug.repo)" ".default_branch")
            if $branch == null { null } else {
              let n = (gh read $fork.name $"repos/($slug.owner)/($slug.repo)/compare/($rev)...($branch)" ".ahead_by")
              if $n == null { null } else { $n | into int }
            }
          }
        )
        let base_date = (
          if $rev == null { null } else if $forge == "github" {
            gh read $fork.name $"repos/($slug.owner)/($slug.repo)/commits/($rev)" ".commit.committer.date"
          } else if $forge == "gitlab" {
            gitlab base-date $fork.url $rev
          } else { null }
        )
        let age_days = (
          if $base_date == null { null } else {
            ((date now) - ($base_date | into datetime)) / 1day | math floor | into int
          }
        )

        # Patch stances walk dag.json node order (the canonical series, same as
        # the sync loop); an unclassified patch defaults to hold (fail-safe).
        let intent = ($fork.patches? | default {})
        let dag_file = ($fork.patchDir | path expand | path join "dag.json")
        let series = (
          if ($dag_file | path exists) { open --raw $dag_file | from json | get nodes | get patch }
          else { $intent | columns }
        )
        let stances = (
          $series | each {|p|
            let mark = ($intent | get -o $p)
            $mark.upstream? | default "hold"
          }
        )
        let retired = ((status load $fork).patches | values | where {|p| $p.retired? | default false } | length)

        # Next-action heuristic, deliberately simple:
        #   retired > 0            -> rebase-shrinks-series: a base bump drops the
        #                             retired patches as empty cherries.
        #   drift fully unknown    -> unknown: no basis to recommend anything.
        #   >= 200 commits behind
        #   or base >= 90 days old -> rebase-recommended: in practice this bites
        #                             the manual pins (nix/clippy/mesa); autoUpdate
        #                             forks are cron-freshened before they get here.
        #   else                   -> ok
        let action = (
          if $retired > 0 { "rebase-shrinks-series" }
          else if $behind == null and $age_days == null { "unknown" }
          else if (($behind | default 0) >= 200) or (($age_days | default 0) >= 90) { "rebase-recommended" }
          else { "ok" }
        )
        {
          name: $fork.name
          forge: $forge
          input: $fork.input
          rev: $rev
          behind: $behind
          baseDate: $base_date
          ageDays: $age_days
          attempt: ($stances | where {|s| $s == "attempt" } | length)
          hold: ($stances | where {|s| $s == "hold" } | length)
          never: ($stances | where {|s| $s == "never" } | length)
          retired: $retired
          action: $action
          note: (
            if $forge == "gitlab" { "base-age only (gitlab compare skipped)" }
            else if $forge == "other" { "unsupported forge" }
            else { "" }
          )
        }
      }

      # `nix run .#upstream-sync -- drift [--json|--markdown] [name]`: the
      # read-only drift report. Network reads only; no status file is written.
      def "main drift" [
        name?: string     # one fork package (nix | btop | ...); all if omitted
        --json            # machine-readable JSON to stdout, nothing else
        --markdown        # GitHub-flavored markdown table (step summaries, PR bodies)
        --mapping: string # fork-package JSON to drive (default: index's baked-in list)
      ] {
        if $json and $markdown {
          error make {msg: "upstream-sync: drift: --json and --markdown are mutually exclusive"}
        }
        let rows = (fork select $name $mapping | each {|fork| drift row $fork })
        if $json {
          print ($rows | to json --indent 2)
          return
        }
        # Human/markdown view: "?" marks an unknown cell (forge unreachable or no
        # locked rev) so a degraded row is visibly degraded, not silently zero.
        let view = (
          $rows | each {|r|
            {
              fork: $r.name
              base: (if $r.rev == null { "?" } else { $r.rev | str substring 0..11 })
              behind: ($r.behind | default "?")
              "age (days)": ($r.ageDays | default "?")
              # action before the stance counts so an 80-column pipe still shows
              # the verdict (nu truncates trailing table columns off-tty).
              action: $r.action
              attempt: $r.attempt
              hold: $r.hold
              never: $r.never
              retired: $r.retired
              note: $r.note
            }
          }
        )
        if $markdown {
          print ($view | to md --pretty)
        } else {
          print $"(ansi cyan)== fork drift: pinned base vs upstream default branch ==(ansi reset)"
          print ($view | table --index false)
        }
      }

      # --- the loop -------------------------------------------------------------

      def main [
        pkg?: string    # one fork package (nix | btop | ...); all if omitted
        patch?: string  # restrict to one patch (name/prefix/unique substring); a series member selects its whole series
        --open          # OPEN real upstream PRs for attempt patches (the outward act). Default: refresh + plan only.
        --dry-run       # plan only: refresh + search but write NO status files (pure validation)
        --check-stale   # additionally warn if a fork has attempt patches but no status file, or a stale lastChecked
        --mapping: string # fork-package JSON to drive (default: index's baked-in list)
      ] {
        # The outward act (opening a PR) is doubly gated, mirroring upstream-pr:
        # the patch must be marked `attempt` in nix (the human intent gate) AND the
        # caller must pass `--open` (the human invocation gate). Without `--open`
        # the tool refreshes tracked PR state, searches duplicates, retires merged
        # patches, writes the status file, and reports which patches WOULD open.
        # `--dry-run` additionally suppresses the status write for pure validation,
        # so the two flags contradict each other; refuse the combination rather
        # than letting a "dry run" perform the outward act.
        if $dry_run and $open {
          error make { msg: "upstream-sync: --dry-run and --open are mutually exclusive; a dry run never opens PRs" }
        }
        let forks = (fork select $pkg $mapping)
        mut plan = []  # accumulate {fork, patch, intent, action, detail} for the summary

        for fork in $forks {
          let slug = (url slug $fork.url)
          let patch_dir = ($fork.patchDir | path expand)
          let policy = ($fork.upstreamPolicy? | default {prsWelcome: true, aiPrsAllowed: "unknown", citation: "", notes: ""})
          # The documented forms are true | "false" | "unknown"; a mapping may use
          # the JSON boolean, so normalize through `into string` (false -> "false")
          # before gating. A missed boolean here would let PRs leak to an AI-banned
          # upstream.
          let ai_allowed = (($policy.aiPrsAllowed? | default "unknown") | into string)
          let intent = ($fork.patches? | default {})

          # Repo-level gates: a non-github host has no gh path; PRs unwelcome or AI
          # banned means we never open here. We still LOAD + report status, but skip
          # any outward act. Reported once per fork so the plan is legible.
          let gh_ok = (is github $fork.url)
          let repo_blocked = (
            (not $policy.prsWelcome) or ($ai_allowed == "false") or (not $gh_ok)
          )
          let repo_block_reason = (
            if not $gh_ok { $"upstream is not GitHub (($slug.owner)/($slug.repo)); gh path N/A" }
            else if not $policy.prsWelcome { "policy: prsWelcome = false" }
            else if ($ai_allowed == "false") { $"policy: aiPrsAllowed = false; see ($policy.citation)" }
            else { "" }
          )

          print $"(ansi cyan)== ($fork.name) [(($slug.owner)/($slug.repo))] ==(ansi reset)"
          if $repo_blocked {
            print $"(ansi yellow)upstream-sync: ($fork.name): repo-level block: ($repo_block_reason). No PR will be opened here.(ansi reset)"
          }
          if $ai_allowed == "unknown" and $gh_ok and $policy.prsWelcome {
            print $"(ansi yellow)upstream-sync: ($fork.name): AI-PR policy is UNSTATED upstream; proceeding for attempt patches with AI attribution in the PR body. Citation: ($policy.citation)(ansi reset)"
          }

          mut doc = (status load $fork)
          # Pre-run committed state, captured before this run touches it, so the
          # `--check-stale` verdict reflects what was actually committed rather
          # than the file this run is about to write.
          let pre_existed = (status path $fork | path exists)
          let pre_last_checked = $doc.lastChecked
          # Only a FULL sync may stamp the fork-level lastChecked: a targeted run
          # (patch selector given) refreshes a subset, and stamping it as a full
          # refresh would mask stale state for every patch it skipped.
          if $patch == null {
            $doc = ($doc | update lastChecked (date now | format date "%Y-%m-%dT%H:%M:%SZ"))
          }

          # The patch set to walk: dag.json node order (canonical). A selector
          # resolves to exactly ONE patch via the shared resolver (ambiguity
          # errors loudly rather than fanning an outward act across matches).
          let dag_file = ($patch_dir | path join "dag.json")
          if not ($dag_file | path exists) {
            print $"(ansi yellow)upstream-sync: ($fork.name): no dag.json; run `nix run .#rebase-patches -- dag ($fork.name)`. Skipping.(ansi reset)"
            continue
          }
          let all_patches = (open --raw $dag_file | from json | get nodes | get patch)
          let selected = (
            if $patch == null { $all_patches } else { [(patch resolve $patch $all_patches)] }
          )

          # Group the walk into UNITS: a `prSeries` intent group is ONE unit (one
          # upstream PR carrying every member; the DAG's textual independence
          # deliberately does not split a semantic feature series into per-commit
          # PRs), everything else is a single-patch unit. Selecting any member of
          # a series acts on the whole series. `uniq` preserves first-occurrence
          # order, so units stay in NNNN order.
          let unit_keys = (
            $all_patches
            | each {|p|
                let s = ($intent | get -o $p | default {} | get -o prSeries)
                if $s == null { $"patch:($p)" } else { $"series:($s)" }
              }
            | uniq
          )
          let units = (
            $unit_keys
            | each {|k|
                if ($k | str starts-with "series:") {
                  let sname = ($k | str replace "series:" "")
                  let members = ($all_patches | where {|p| ($intent | get -o $p | default {} | get -o prSeries) == $sname })
                  {series: $sname, patches: $members}
                } else {
                  {series: null, patches: [($k | str replace "patch:" "")]}
                }
              }
            | where {|u| $u.patches | any {|p| $p in $selected } }
          )

          for unit in $units {
            # A series must be intent-coherent: uniform mark, exactly one head.
            # The patch-dag-<name> check enforces this in CI; failing loudly here
            # too keeps a downstream mapping without that check honest.
            let marks = (
              $unit.patches
              | each {|p| $intent | get -o $p | default {} | get -o upstream | default "hold" }
              | uniq
            )
            if ($marks | length) > 1 {
              error make { msg: $"upstream-sync: ($fork.name): series ($unit.series) has mixed upstream marks [($marks | str join ', ')]; make them uniform in the fork mapping" }
            }
            let upstream = ($marks | first)
            let head = (
              if $unit.series == null { $unit.patches | first } else {
                let heads = ($unit.patches | where {|p| $intent | get -o $p | default {} | get -o prSeriesHead | default false })
                if ($heads | length) != 1 {
                  error make { msg: $"upstream-sync: ($fork.name): series ($unit.series) must declare exactly one prSeriesHead; found (($heads | length))" }
                }
                $heads | first
              }
            )
            let label = (
              if $unit.series == null { $head } else {
                $"($head) [series ($unit.series): (($unit.patches | length)) patches]"
              }
            )
            if $unit.series != null and $patch != null {
              print $"(ansi yellow)upstream-sync: ($fork.name): selector matched a member of series ($unit.series); acting on the whole series.(ansi reset)"
            }
            let reason = ($intent | get -o $head | default {} | get -o reason | default "unclassified (no intent entry in lib/fork-packages.nix)")

            # Ensure a status entry per member (mirror intent for legibility).
            # reduce closures capture only immutable lets (nushell forbids mut).
            let seeded = (
              $unit.patches | reduce --fold $doc.patches {|nm, acc|
                let m = ($intent | get -o $nm | default {} | get -o upstream | default "hold")
                $acc | upsert $nm (
                  ($acc | get -o $nm | default {upstream: $m, pr: null, retired: false, duplicates: []})
                  | update upstream $m
                )
              }
            )
            $doc = ($doc | upsert patches $seeded)

            # 1. Refresh a tracked PR FIRST, regardless of intent or repo blocks:
            # intent gates only NEW outward acts, and refreshing is read-only.
            # Without this, demoting an already-opened patch to hold/never would
            # freeze its status forever and its merge would never retire it.
            # Series members share one PR record; the head's is authoritative.
            let tracked = ($doc.patches | get $head | get pr)
            if $tracked != null {
              let fresh = (pr refresh $slug $tracked.number)
              if $fresh == null {
                $doc = (log append $doc $"($label): tracked PR #($tracked.number) no longer readable, deleted or renamed; leaving last-known state")
                $plan = ($plan | append {fork: $fork.name, patch: $label, intent: $upstream, action: "stale-pr", detail: $"PR #($tracked.number) unreadable"})
                continue
              }
              if $fresh.state != $tracked.state {
                $doc = (log append $doc $"($label): PR #($fresh.number) ($tracked.state) -> ($fresh.state) (($fresh.url))")
              }
              # Merged upstream -> retire every member. The next base bump's
              # rebase-patches run should drop them as empty cherries; the plan
              # notes it for a human/agent to verify.
              let retire = ($fresh.state == "merged")
              if $retire and (not ($doc.patches | get $head | get retired)) {
                $doc = (log append $doc $"($label): merged upstream in PR #($fresh.number); marked retired. Verify the next base bump drops it as an empty cherry.")
              }
              let updated = (
                $unit.patches | reduce --fold $doc.patches {|nm, acc|
                  let cur = (($acc | get $nm) | update pr $fresh)
                  $acc | upsert $nm (if $retire { $cur | update retired true } else { $cur })
                }
              )
              $doc = ($doc | upsert patches $updated)
              let action = (if $retire { "retired" } else { $"tracked:($fresh.state)" })
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: $upstream, action: $action, detail: $fresh.url})
              continue
            }

            if $upstream != "attempt" {
              # Not authorized for the outward act; record intent, no action.
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: $upstream, action: "skip", detail: $reason})
              continue
            }

            # attempt unit. Repo-level block still wins (defense in depth).
            if $repo_blocked {
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "blocked", detail: $repo_block_reason})
              continue
            }

            # 2. No tracked PR: search for a duplicate before opening, keyed on
            # the head's (unfolded) subject.
            let subject = (patch subject ($patch_dir | path join $head))
            let dupes = (pr find-duplicates $slug $subject)
            if ($dupes | is-not-empty) {
              $doc = ($doc | upsert patches ($doc.patches | upsert $head (($doc.patches | get $head) | update duplicates $dupes)))
              $doc = (log append $doc $"($label): found (($dupes | length)) possible duplicate upstream PRs; NOT opening. First: (($dupes | first).url)")
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "duplicate", detail: (($dupes | first).url)})
              continue
            }

            # 3. No PR, no duplicate: open one ONLY when --open was passed.
            # Without it (the safe default) this is a would-open plan entry: the
            # status file still records the pending attempt, but no PR is created.
            let open_cmd = $"upstream-pr --open --message-from ($head) ($fork.name) (($unit.patches | str join ' '))"
            if not $open {
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "would-open", detail: $"run with --open to create: ($open_cmd)"})
              continue
            }

            # Closure-gate preflight (RFC 0010 A3, #2098): `upstream-pr` ships
            # the unit as its dag.json ancestor closure against the bare base,
            # so for forks opted in via `closureGates = true` prove every
            # member's closure BUILDS before the outward act, and abort THIS
            # unit's PR-opening on a red gate (series members are textually
            # independent in the DAG, so each member has its own gate attr).
            # The gate attr is the current repo flake's (a downstream --mapping
            # repo gates against its own flake; without the flag no fork ever
            # pays this build).
            let gates_on = ($fork.closureGates? | default false)
            mut gate_failed: any = null
            if $gates_on {
              let system = (nix config show system | str trim)
              for p in $unit.patches {
                if $gate_failed != null { continue }
                let gate = $".#forkClosureGates.($system).($fork.name).\"($p)\""
                print $"(ansi cyan)upstream-sync: ($fork.name): building closure gate ($gate) before opening \(heavy full-package build; cache hit when unchanged)(ansi reset)"
                let gate_res = (do { nix build --no-link $gate } | complete)
                if $gate_res.exit_code != 0 {
                  print ($gate_res.stderr)
                  print $"(ansi red)upstream-sync: ($fork.name): closure gate FAILED for ($p): its dag.json closure does not build standalone, so the upstream PR would ship broken. Fix the series; NOT opening.(ansi reset)"
                  $gate_failed = $gate
                }
              }
            }
            if $gate_failed != null {
              $doc = (log append $doc $"($label): closure gate build FAILED; PR-opening aborted")
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "gate-failed", detail: $gate_failed})
              continue
            }

            # The outward act, only for attempt units on a non-blocked repo, only
            # when --open was passed. upstream-pr owns the branch/am/push/draft-PR
            # mechanism (one PR for the whole unit); --mapping is threaded so a
            # downstream repo's list is used.
            print $"(ansi green)upstream-sync: ($fork.name): opening upstream PR for ($label) via ($open_cmd)(ansi reset)"
            let args = (
              ["--open" "--message-from" $head]
              | append (if $mapping != null { ["--mapping" $mapping] } else { [] })
              | append [$fork.name]
              | append $unit.patches
            )
            let opened = (do { ^upstream-pr ...$args } | complete)
            print ($opened.stdout)
            if $opened.exit_code != 0 {
              print $"(ansi red)upstream-sync: ($fork.name): upstream-pr failed for ($label):(ansi reset)"
              print ($opened.stderr)
              $doc = (log append $doc $"($label): upstream-pr --open FAILED; see output above")
              $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "open-failed", detail: "upstream-pr error"})
              continue
            }
            # Parse the created PR URL from upstream-pr's output (gh prints it on
            # `pr create`). Best-effort: if we cannot parse it, still log the act.
            # Each `str contains` is parenthesized (it would otherwise greedily
            # swallow the `and`); `last` on an empty list yields null, which the
            # guard below handles.
            let pr_url = (
              $opened.stdout
              | lines
              | where {|l| ($l | str contains "github.com") and ($l | str contains "/pull/") }
              | last
            )
            if $pr_url != null {
              let pr_num = ($pr_url | parse --regex '/pull/(?<n>[0-9]+)' | get n.0? | default "0" | into int)
              let fresh = {url: ($pr_url | str trim), number: $pr_num, state: "draft", checkedAt: (date now | format date "%Y-%m-%dT%H:%M:%SZ")}
              let recorded = (
                $unit.patches | reduce --fold $doc.patches {|nm, acc|
                  $acc | upsert $nm (($acc | get $nm) | update pr $fresh)
                }
              )
              $doc = ($doc | upsert patches $recorded)
              $doc = (log append $doc $"($label): opened draft PR ($fresh.url)")
            } else {
              $doc = (log append $doc $"($label): upstream-pr --open succeeded but PR URL was not parseable from output")
            }
            $plan = ($plan | append {fork: $fork.name, patch: $label, intent: "attempt", action: "opened", detail: ($pr_url | default "unknown")})
          }

          status save $fork $doc $dry_run

          # Staleness verdicts judge the PRE-run committed state (captured at
          # load), so they are meaningful in every mode, including right after
          # this run wrote a fresh file.
          if $check_stale {
            let attempts = ($intent | items {|k, v| $v.upstream? | default "hold" } | where {|s| $s == "attempt" } | length)
            if $attempts > 0 and (not $pre_existed) {
              print $"(ansi yellow)upstream-sync: ($fork.name): STALE: has ($attempts) attempt patches but no committed upstream-status.json; run a non-dry-run sync and commit it.(ansi reset)"
            } else if $pre_last_checked != null {
              # 14 days: tracked-PR state and the duplicate landscape move on the
              # scale of weeks; older than that and the committed state is a stale
              # basis for the next upstreaming decision.
              let age = ((date now) - ($pre_last_checked | into datetime))
              if $age > 14day {
                print $"(ansi yellow)upstream-sync: ($fork.name): STALE: committed upstream-status.json was last checked ($pre_last_checked), ($age) ago; re-run and commit.(ansi reset)"
              }
            }
          }
        }

        # --- plan summary ----------------------------------------------------
        # Grouped by action, one patch per line with its full detail (no table
        # truncation), so the output pastes straight into a PR body / plan review.
        print ""
        print $"(ansi cyan)== upstream-sync plan: (($plan | length)) patch decisions ==(ansi reset)"
        if ($plan | is-empty) {
          print "  (no patches selected)"
        } else {
          for grp in ($plan | group-by action | transpose action rows) {
            print $"(ansi cyan)[($grp.action)] (($grp.rows | length))(ansi reset)"
            for r in $grp.rows {
              print $"  ($r.fork) / ($r.patch)"
              print $"      ($r.detail)"
            }
          }
        }
        let ready = ($plan | where {|r| $r.action == "would-open" or $r.action == "opened" })
        if ($ready | is-not-empty) {
          print ""
          print $"(ansi green)attempt-ready patches (($ready | length)): these are the outward-PR candidates.(ansi reset)"
          for r in $ready { print $"  - ($r.fork) / ($r.patch)" }
          if not $open {
            print $"(ansi yellow)Re-run with --open to create these PRs; opening is the outward act.(ansi reset)"
          }
        }
        if $dry_run {
          print ""
          print $"(ansi yellow)--dry-run: no status files written. Drop --dry-run to persist the refreshed status; add --open to create PRs.(ansi reset)"
        }
      }
    '';
  };

  # Hermetic lifecycle test for the branch no other check reaches: the --open
  # recording path and the merged->retired transition run ONLY after a real
  # upstream PR exists, so a bug there surfaces on first outward use, orphaning
  # an opened-but-untracked PR and inviting a duplicate on the next run (this
  # exact failure shipped once: an invalid `last?` command and an unparenthesized
  # `str contains ... and ...` both parsed fine at build time and crashed at
  # runtime). gh and upstream-pr are stubbed, so the whole PR lifecycle runs in
  # the sandbox with no network: open + record, merged -> retired, idempotent
  # re-run. The test drives the REAL shipped script (the wrapper's PATH preamble
  # pins store paths ahead of the stubs, so the body is extracted from the `# nu`
  # marker on and run under a stub-first PATH).
  lifecycle = runCommand "upstream-sync-lifecycle-test" {nativeBuildInputs = [nushell];} ''
    mkdir -p stubs work/repo/patches
    export HOME="$PWD"

    # Stub gh: the search phase finds no duplicates; the view phase replays
    # whatever PR state the stage under test staged into GH_PR_VIEW_RESPONSE.
    # Dollars are escaped so the heredoc expands only $(command -v bash) now.
    cat > stubs/gh <<STUB
    #!$(command -v bash)
    case "\$1 \$2" in
      "search prs") echo "[]" ;;
      "pr view") cat "\$GH_PR_VIEW_RESPONSE" ;;
      *) echo "stub gh: unexpected: \$*" >&2; exit 1 ;;
    esac
    STUB

    # Stub upstream-pr: mimic the real output shape (compare URL, then the
    # `gh pr create` URL line the parser must pick) and journal each invocation
    # so the test can assert the series unit arrives as ONE call with every
    # member and --message-from the head.
    cat > stubs/upstream-pr <<STUB
    #!$(command -v bash)
    echo "\$*" >> "\$UPSTREAM_PR_CALLS"
    echo "upstream-pr: stub invoked with: \$*"
    echo "  https://github.com/fakeorg/fakerepo/compare/main...indexable-inc:fakerepo:branch?expand=1"
    echo "https://github.com/fakeorg/fakerepo/pull/99999"
    STUB

    # Stub nix, for the closure-gate preflight: `config show system` names the
    # gate attr's system, `build` exits per NIX_GATE_EXIT so the stages below
    # drive a red and a green gate through the REAL preflight branch.
    cat > stubs/nix <<STUB
    #!$(command -v bash)
    case "\$1" in
      config) echo "x86_64-stub" ;;
      build) exit "\''${NIX_GATE_EXIT:-0}" ;;
      *) echo "stub nix: unexpected: \$*" >&2; exit 1 ;;
    esac
    STUB
    chmod +x stubs/gh stubs/upstream-pr stubs/nix

    # Three patches: 0001 is a single-patch unit; 0002+0003 form a prSeries
    # ("frob-suite") whose head is 0003, exercising the one-PR-per-series path.
    for p in "0001-fake-fix:repair the frobnicator widget alignment" \
             "0002-frob-core:add the frob suite core" \
             "0003-frob-cli:add the frob suite cli"; do
      name="''${p%%:*}"; subj="''${p#*:}"
      cat > "work/repo/patches/$name.patch" <<EOF
    From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
    From: Test <t@t>
    Date: Mon, 1 Jan 2026 00:00:00 +0000
    Subject: [PATCH] $subj

    ---
    EOF
    done
    nu -c '{comment: "t", base: "deadbeef", nodes: [
      {patch: "0001-fake-fix.patch", deps: []},
      {patch: "0002-frob-core.patch", deps: []},
      {patch: "0003-frob-cli.patch", deps: []}
    ]} | to json | save work/repo/patches/dag.json'
    cat > work/mapping.json <<'EOF'
    [{"name":"fake","input":"fake-src","url":"https://github.com/fakeorg/fakerepo.git",
      "patchDir":"repo/patches","autoUpdate":false,
      "upstreamPolicy":{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"},
      "patches":{
        "0001-fake-fix.patch":{"upstream":"attempt","reason":"lifecycle test"},
        "0002-frob-core.patch":{"upstream":"attempt","reason":"series member","prSeries":"frob-suite"},
        "0003-frob-cli.patch":{"upstream":"attempt","reason":"series head","prSeries":"frob-suite","prSeriesHead":true}
      }}]
    EOF

    # The shipped script body, from the `# nu` marker on (past the PATH preamble).
    awk '/^# nu$/,0' ${package}/bin/upstream-sync > script.nu
    export PATH="$PWD/stubs:$PATH"
    export UPSTREAM_PR_CALLS="$PWD/upstream-pr-calls.log"
    cd work

    echo "--- stage 1: --open records the created PRs (single + series-as-one) ---"
    nu ../script.nu --open --mapping "$PWD/mapping.json" fake
    nu -c '
      let d = (open repo/patches/upstream-status.json)
      for nm in ["0001-fake-fix.patch" "0002-frob-core.patch" "0003-frob-cli.patch"] {
        let p = ($d.patches | get $nm)
        if $p.pr.number != 99999 or $p.pr.state != "draft" or $p.retired {
          error make {msg: $"stage 1: PR not recorded on ($nm): ($p | to json)"}
        }
      }
      let calls = (open --raw $env.UPSTREAM_PR_CALLS | lines)
      if ($calls | length) != 2 {
        error make {msg: $"stage 1: expected 2 upstream-pr calls \(single + series\), got: ($calls | to json)"}
      }
      # The series call carries BOTH members and --message-from its head.
      let series_call = ($calls | where {|c| $c | str contains "0002-frob-core.patch" } | first)
      if not (($series_call | str contains "0003-frob-cli.patch") and ($series_call | str contains "--message-from 0003-frob-cli.patch")) {
        error make {msg: $"stage 1: series call malformed: ($series_call)"}
      }'

    echo "--- stage 2: merged upstream -> every member retired ---"
    echo '{"state":"MERGED","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999}' > pr-view.json
    export GH_PR_VIEW_RESPONSE="$PWD/pr-view.json"
    nu ../script.nu --mapping "$PWD/mapping.json" fake
    nu -c '
      let d = (open repo/patches/upstream-status.json)
      for nm in ["0001-fake-fix.patch" "0002-frob-core.patch" "0003-frob-cli.patch"] {
        let p = ($d.patches | get $nm)
        if $p.pr.state != "merged" or (not $p.retired) {
          error make {msg: $"stage 2: ($nm) not retired: ($p | to json)"}
        }
      }
      # 2 opened + per-unit (state change + retire) x 2 units = 6 transitions.
      if ($d.log | length) != 6 {
        error make {msg: $"stage 2: expected 6 log transitions, got ($d.log | to json)"}
      }'

    echo "--- stage 3: re-run is idempotent (no duplicate transitions) ---"
    nu ../script.nu --mapping "$PWD/mapping.json" fake
    nu -c '
      let d = (open repo/patches/upstream-status.json)
      if ($d.log | length) != 6 {
        error make {msg: $"stage 3: log grew on a no-change re-run: ($d.log | to json)"}
      }'

    # A closureGates fork: same patch/dag shape, its own patch dir + status
    # file, exercising the preflight branch (RFC 0010 A3) that otherwise runs
    # only on a real --open against a real flake.
    mkdir -p gated/patches
    cp repo/patches/0001-fake-fix.patch repo/patches/dag.json gated/patches/
    cat > mapping-gated.json <<'EOF'
    [{"name":"gated","input":"gated-src","url":"https://github.com/fakeorg/fakerepo.git",
      "patchDir":"gated/patches","autoUpdate":false,"closureGates":true,
      "upstreamPolicy":{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"},
      "patches":{"0001-fake-fix.patch":{"upstream":"attempt","reason":"gate test"}}}]
    EOF

    echo "--- stage 4: a red closure gate aborts the PR-opening ---"
    NIX_GATE_EXIT=1 nu ../script.nu --open --mapping "$PWD/mapping-gated.json" gated
    nu -c '
      let p = (open gated/patches/upstream-status.json | get patches."0001-fake-fix.patch")
      if $p.pr != null {
        error make {msg: $"stage 4: PR opened despite a failed gate: ($p | to json)"}
      }'

    echo "--- stage 5: a green gate proceeds to open and record the PR ---"
    NIX_GATE_EXIT=0 nu ../script.nu --open --mapping "$PWD/mapping-gated.json" gated
    nu -c '
      let p = (open gated/patches/upstream-status.json | get patches."0001-fake-fix.patch")
      if $p.pr.number != 99999 {
        error make {msg: $"stage 5: PR not recorded after a green gate: ($p | to json)"}
      }'

    touch "$out"
  '';
  # Drift-report test for the pure parts no live run pins deterministically:
  # flake.lock rev extraction, stance + retired counting, the next-action
  # heuristic, and the degrade path (a failing forge yields unknown cells and a
  # zero exit, never a crashed report). gh is stubbed with fixed `--jq`'d
  # responses; nothing elaborate, the forge is not what is under test.
  drift = runCommand "upstream-sync-drift-test" {nativeBuildInputs = [nushell];} ''
    mkdir -p stubs work/repo/patches work/bad/patches
    export HOME="$PWD"

    # Stub gh: `gh api <path> --jq <expr>` keyed on the path. Every bad-fork
    # endpoint fails, exercising degrade-to-unknown.
    cat > stubs/gh <<STUB
    #!$(command -v bash)
    case "\$2" in
      repos/fakeorg/fakerepo) echo "main" ;;
      repos/fakeorg/fakerepo/compare/*) echo "123" ;;
      repos/fakeorg/fakerepo/commits/*) echo "2026-01-01T00:00:00Z" ;;
      *) echo "stub gh: unexpected: \$*" >&2; exit 1 ;;
    esac
    STUB
    chmod +x stubs/gh

    cat > work/flake.lock <<'EOF'
    {"nodes": {"fake-src": {"locked": {"rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
               "bad-src": {"locked": {"rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}}
    EOF
    echo '{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-sent.patch","deps":[]},{"patch":"0002-kept.patch","deps":[]},{"patch":"0003-unclassified.patch","deps":[]}]}' \
      > work/repo/patches/dag.json
    cat > work/repo/patches/upstream-status.json <<'EOF'
    {"comment":"t","lastChecked":"2026-01-01T00:00:00Z","patches":{"0001-sent.patch":{"upstream":"attempt","pr":{"url":"u","number":1,"state":"merged","checkedAt":"t"},"retired":true,"duplicates":[]}},"log":[]}
    EOF
    echo '{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-x.patch","deps":[]}]}' > work/bad/patches/dag.json
    cat > work/mapping.json <<'EOF'
    [{"name":"fake","input":"fake-src","url":"https://github.com/fakeorg/fakerepo.git",
      "patchDir":"repo/patches","autoUpdate":false,
      "patches":{"0001-sent.patch":{"upstream":"attempt","reason":"t"},"0002-kept.patch":{"upstream":"never","reason":"t"}}},
     {"name":"bad","input":"bad-src","url":"https://github.com/badorg/badrepo.git",
      "patchDir":"bad/patches","autoUpdate":false,"patches":{}}]
    EOF

    awk '/^# nu$/,0' ${package}/bin/upstream-sync > script.nu
    export PATH="$PWD/stubs:$PATH"
    cd work

    # --json is the machine surface: stdout must parse as JSON alone (warnings
    # go to stderr), the fake row carries the stubbed forge facts plus the
    # retired-driven action, and the bad row (gh failing) is unknown, not fatal.
    nu ../script.nu drift --json --mapping "$PWD/mapping.json" > drift.json
    nu -c '
      let rows = (open --raw drift.json | from json)
      let fake = ($rows | where name == "fake" | first)
      if $fake.rev != "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" or $fake.behind != 123 or $fake.ageDays < 1 {
        error make {msg: $"fake drift facts: ($fake | to json)"}
      }
      if $fake.attempt != 1 or $fake.hold != 1 or $fake.never != 1 or $fake.retired != 1 {
        error make {msg: $"fake stance counts: ($fake | to json)"}
      }
      if $fake.action != "rebase-shrinks-series" {
        error make {msg: $"fake action: ($fake | to json)"}
      }
      let bad = ($rows | where name == "bad" | first)
      if $bad.behind != null or $bad.ageDays != null or $bad.action != "unknown" {
        error make {msg: $"bad row should degrade to unknown: ($bad | to json)"}
      }'

    # The markdown surface renders every fork as a table row, "?" for unknowns.
    nu ../script.nu drift --markdown --mapping "$PWD/mapping.json" > drift.md
    nu -c '
      let md = (open --raw drift.md)
      if not ((($md | str contains "| fake") and ($md | str contains "| bad")) and ($md | str contains "?")) {
        error make {msg: $"markdown table missing rows: ($md)"}
      }'

    touch "$out"
  '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = (old.passthru.tests or {}) // {inherit drift lifecycle;};
      };
  })
