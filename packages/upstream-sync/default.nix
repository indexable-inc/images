# `nix run .#upstream-sync [-- <pkg> [<patch>]] [--open] [--dry-run] [--check-stale]`:
# drive the de-fork UPSTREAMING loop. This is the layer above `upstream-pr` (the
# per-patch branch/am/push/PR mechanism) and `rebase-patches` (the base-bump
# regenerator): it decides which patches to act on from the hand-written
# declarative intent, tracks the live state of the PRs we open, spots duplicate
# upstream PRs, and retires patches that land upstream.
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
# The loop, per `attempt` patch of each selected fork:
#   1. If we already track a PR: refresh its state via `gh pr view` (open / draft
#      / merged / closed). If merged, mark `retired = true` and record it: the
#      NEXT base bump's `rebase-patches` run should drop the patch (it becomes an
#      empty cherry against the new base), and this tool wires a retirement note
#      into the plan so a human/agent verifies the drop.
#   2. Else search the upstream repo for a DUPLICATE/related PR by the patch's
#      title keywords. If found, RECORD it and SKIP loudly (a human or agent can
#      comment on the existing PR instead of opening a competing one).
#   3. Else, if `--open` was passed, open the PR by delegating to
#      `upstream-pr --open` (its DAG-closure/am/push/draft-PR mechanism, one owner).
#      The PR body carries AI attribution (outward-message policy) and a link back
#      to our patch file. Opening a PR is the outward act, DOUBLY gated: the patch
#      must be marked `attempt` in nix (intent gate) AND `--open` must be passed
#      (invocation gate). Without `--open`, the safe default, the tool still
#      refreshes/searches/retires and writes the status file, and reports which
#      patches WOULD open. `--dry-run` additionally suppresses the status write.
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
  # Sibling repo packages, threaded under one name (see lib/packages.nix); we take
  # the PR mechanism (`upstream-pr`) from here rather than a bare callPackage arg,
  # which the package set does not expose flat.
  repoPackages,
  runCommand,
  nushell,
}: let
  inherit (repoPackages) upstream-pr;
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
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
      upstream-pr
    ];
    text = ''
      # nu
      # Run from the repo root: `nix run .#upstream-sync [-- <pkg> [<patch>]]`.
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
      # to record and skip rather than open a competing one. gh's PR search is a
      # fuzzy OR over tokens, so we (1) query only distinctive title tokens with
      # `in:title`, then (2) post-filter to hits whose title shares >= 2 of our
      # distinctive tokens. This trades a few missed near-matches for far fewer
      # false positives (the skip is conservative-safe: a real dup we miss just
      # gets an extra PR a human can dedupe, whereas a false dup that BLOCKS an
      # attempt is a silent no-op we do NOT want). Best-effort: any failure or a
      # tokenless subject returns [] so the loop never stalls.
      def "pr find-duplicates" [slug: record, subject: string]: nothing -> list<record> {
        let tokens = (subject tokens $subject)
        if ($tokens | is-empty) { return [] }
        let query = $"(($tokens | str join ' ')) in:title"
        let res = (
          do {
            gh search prs $query --repo $"($slug.owner)/($slug.repo)" --state open --limit 20 --json url,number,title
          } | complete
        )
        if $res.exit_code != 0 { return [] }
        ($res.stdout | from json)
        | where {|hit|
          let ht = (subject tokens $hit.title)
          (($tokens | where {|t| $t in $ht }) | length) >= 2
        }
      }

      # The subject line of a patch file (its commit message summary), used both as
      # duplicate-search keywords and to describe the patch in the plan.
      def "patch subject" [patch_dir: string, patch: string]: nothing -> string {
        let f = ($patch_dir | path join $patch)
        open --raw $f
        | lines
        | where {|l| $l | str starts-with "Subject:" }
        | first
        | default "Subject: (none)"
        | str replace --regex '^Subject:\s*(\[PATCH[^\]]*\]\s*)?' ""
      }

      # --- the loop -------------------------------------------------------------

      def main [
        pkg?: string    # one fork package (nix | btop | ...); all if omitted
        patch?: string  # restrict to one patch file (name/prefix/substring); optional
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
        # `--dry-run` additionally suppresses the status write for pure validation.
        let forks = (fork select $pkg $mapping)
        mut plan = []  # accumulate {fork, patch, intent, action, detail} for the summary

        for fork in $forks {
          let slug = (url slug $fork.url)
          let patch_dir = ($fork.patchDir | path expand)
          let policy = ($fork.upstreamPolicy? | default {prsWelcome: true, aiPrsAllowed: "unknown", citation: "", notes: ""})
          let intent = ($fork.patches? | default {})

          # Repo-level gates: a non-github host has no gh path; PRs unwelcome or AI
          # banned means we never open here. We still LOAD + report status, but skip
          # any outward act. Reported once per fork so the plan is legible.
          let gh_ok = (is github $fork.url)
          let repo_blocked = (
            (not $policy.prsWelcome) or ($policy.aiPrsAllowed == "false") or (not $gh_ok)
          )
          let repo_block_reason = (
            if not $gh_ok { $"upstream is not GitHub (($slug.owner)/($slug.repo)); gh path N/A" }
            else if not $policy.prsWelcome { "policy: prsWelcome = false" }
            else if ($policy.aiPrsAllowed == "false") { $"policy: aiPrsAllowed = false; see ($policy.citation)" }
            else { "" }
          )

          print $"(ansi cyan)== ($fork.name) [(($slug.owner)/($slug.repo))] ==(ansi reset)"
          if $repo_blocked {
            print $"(ansi yellow)upstream-sync: ($fork.name): repo-level block: ($repo_block_reason). No PR will be opened here.(ansi reset)"
          }
          if $policy.aiPrsAllowed == "unknown" and $gh_ok and $policy.prsWelcome {
            print $"(ansi yellow)upstream-sync: ($fork.name): AI-PR policy is UNSTATED upstream; proceeding for attempt patches with AI attribution in the PR body. Citation: ($policy.citation)(ansi reset)"
          }

          mut doc = (status load $fork)
          # Pre-run committed state, captured before this run touches it, so the
          # `--check-stale` verdict reflects what was actually committed rather
          # than the file this run is about to write.
          let pre_existed = (status path $fork | path exists)
          let pre_last_checked = $doc.lastChecked
          $doc = ($doc | update lastChecked (date now | format date "%Y-%m-%dT%H:%M:%SZ"))

          # The patch set to walk: dag.json node order (canonical), filtered by the
          # optional `patch` arg.
          let dag_file = ($patch_dir | path join "dag.json")
          if not ($dag_file | path exists) {
            print $"(ansi yellow)upstream-sync: ($fork.name): no dag.json; run `nix run .#rebase-patches -- dag ($fork.name)`. Skipping.(ansi reset)"
            continue
          }
          let all_patches = (open --raw $dag_file | from json | get nodes | get patch)
          let selected_patches = (
            if $patch == null { $all_patches } else {
              $all_patches | where {|p| ($p == $patch) or ($p | str starts-with $patch) or ($p | str contains $patch) }
            }
          )
          if ($selected_patches | is-empty) and ($patch != null) {
            print $"(ansi yellow)upstream-sync: ($fork.name): no patch matching '($patch)'.(ansi reset)"
          }

          for pf in $selected_patches {
            let mark = ($intent | get -o $pf)
            # Fail-safe default: an unclassified patch is `hold`, never sent.
            let upstream = ($mark.upstream? | default "hold")
            let reason = ($mark.reason? | default "unclassified (no intent entry in lib/fork-packages.nix)")

            # Ensure a status entry exists (mirror intent for legibility).
            let existing = ($doc.patches | get -o $pf)
            let entry = (
              $existing | default {upstream: $upstream, pr: null, retired: false, duplicates: []}
              | update upstream $upstream
            )
            # Set the patch entry (upsert: the key may already exist from status
            # load). The value pipeline is evaluated first, so nothing mutable is
            # captured in a closure (nushell forbids that).
            $doc = ($doc | upsert patches ($doc.patches | upsert $pf $entry))

            if $upstream != "attempt" {
              # Not authorized for the outward act; record intent, no action.
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: $upstream, action: "skip", detail: $reason})
              continue
            }

            # attempt patch. Repo-level block still wins (defense in depth).
            if $repo_blocked {
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "blocked", detail: $repo_block_reason})
              continue
            }

            # 1. Already tracking a PR? Refresh its state.
            let tracked = ($doc.patches | get $pf | get pr)
            if $tracked != null {
              let fresh = (pr refresh $slug $tracked.number)
              if $fresh == null {
                $doc = (log append $doc $"($pf): tracked PR #($tracked.number) no longer readable, deleted or renamed; leaving last-known state")
                $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "stale-pr", detail: $"PR #($tracked.number) unreadable"})
                continue
              }
              # Log a state transition when it changed.
              if $fresh.state != $tracked.state {
                $doc = (log append $doc $"($pf): PR #($fresh.number) ($tracked.state) -> ($fresh.state) (($fresh.url))")
              }
              mut new_entry = ($doc.patches | get $pf | update pr $fresh)
              # 1b. Merged upstream -> retire. Next base bump's rebase-patches run
              # should drop the patch (it cherries empty against the new base); we
              # wire that verification into the plan for a human/agent to confirm.
              if $fresh.state == "merged" and (not ($doc.patches | get $pf | get retired)) {
                $new_entry = ($new_entry | update retired true)
                $doc = (log append $doc $"($pf): merged upstream in PR #($fresh.number); marked retired. Verify the next base bump drops it as an empty cherry.")
              }
              $doc = ($doc | upsert patches ($doc.patches | upsert $pf $new_entry))
              let action = (if $fresh.state == "merged" { "retired" } else { $"tracked:($fresh.state)" })
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: $action, detail: $fresh.url})
              continue
            }

            # 2. No tracked PR: search for a duplicate before opening.
            let subject = (patch subject $patch_dir $pf)
            let dupes = (pr find-duplicates $slug $subject)
            if ($dupes | is-not-empty) {
              $doc = ($doc | upsert patches ($doc.patches | upsert $pf (($doc.patches | get $pf) | update duplicates $dupes)))
              $doc = (log append $doc $"($pf): found (($dupes | length)) possible duplicate upstream PRs; NOT opening. First: (($dupes | first).url)")
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "duplicate", detail: (($dupes | first).url)})
              continue
            }

            # 3. No PR, no duplicate: open one ONLY when --open was passed.
            # Without it (the safe default) this is a would-open plan entry: the
            # status file still records the pending attempt, but no PR is created.
            if not $open {
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "would-open", detail: $"run with --open to create: upstream-pr --open ($fork.name) ($pf)"})
              continue
            }

            # The outward act, only for attempt patches on a non-blocked repo, only
            # when --open was passed. upstream-pr owns the branch/am/push/draft-PR
            # mechanism; --mapping is threaded so a downstream repo's list is used.
            print $"(ansi green)upstream-sync: ($fork.name): opening upstream PR for ($pf) via upstream-pr --open(ansi reset)"
            let args = (
              ["--open"]
              | append (if $mapping != null { ["--mapping" $mapping] } else { [] })
              | append [$fork.name $pf]
            )
            let opened = (do { ^upstream-pr ...$args } | complete)
            print ($opened.stdout)
            if $opened.exit_code != 0 {
              print $"(ansi red)upstream-sync: ($fork.name): upstream-pr failed for ($pf):(ansi reset)"
              print ($opened.stderr)
              $doc = (log append $doc $"($pf): upstream-pr --open FAILED; see output above")
              $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "open-failed", detail: "upstream-pr error"})
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
              $doc = ($doc | upsert patches ($doc.patches | upsert $pf (($doc.patches | get $pf) | update pr $fresh)))
              $doc = (log append $doc $"($pf): opened draft PR ($fresh.url)")
            } else {
              $doc = (log append $doc $"($pf): upstream-pr --open succeeded but PR URL was not parseable from output")
            }
            $plan = ($plan | append {fork: $fork.name, patch: $pf, intent: "attempt", action: "opened", detail: ($pr_url | default "unknown")})
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
    # `gh pr create` URL line the parser must pick).
    cat > stubs/upstream-pr <<STUB
    #!$(command -v bash)
    echo "upstream-pr: stub invoked with: \$*"
    echo "  https://github.com/fakeorg/fakerepo/compare/main...indexable-inc:fakerepo:branch?expand=1"
    echo "https://github.com/fakeorg/fakerepo/pull/99999"
    STUB
    chmod +x stubs/gh stubs/upstream-pr

    cat > work/repo/patches/0001-fake-fix.patch <<'EOF'
    From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001
    From: Test <t@t>
    Date: Mon, 1 Jan 2026 00:00:00 +0000
    Subject: [PATCH] fakefix: repair the frobnicator widget alignment

    ---
    EOF
    echo '{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-fake-fix.patch","deps":[]}]}' \
      > work/repo/patches/dag.json
    cat > work/mapping.json <<'EOF'
    [{"name":"fake","input":"fake-src","url":"https://github.com/fakeorg/fakerepo.git",
      "patchDir":"repo/patches","autoUpdate":false,
      "upstreamPolicy":{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"},
      "patches":{"0001-fake-fix.patch":{"upstream":"attempt","reason":"lifecycle test"}}}]
    EOF

    # The shipped script body, from the `# nu` marker on (past the PATH preamble).
    awk '/^# nu$/,0' ${package}/bin/upstream-sync > script.nu
    export PATH="$PWD/stubs:$PATH"
    cd work

    echo "--- stage 1: --open records the created PR ---"
    nu ../script.nu --open --mapping "$PWD/mapping.json" fake
    nu -c '
      let p = (open repo/patches/upstream-status.json | get patches."0001-fake-fix.patch")
      if $p.pr.number != 99999 or $p.pr.state != "draft" or $p.retired {
        error make {msg: $"stage 1: PR not recorded: ($p | to json)"}
      }'

    echo "--- stage 2: merged upstream -> retired ---"
    echo '{"state":"MERGED","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999}' > pr-view.json
    export GH_PR_VIEW_RESPONSE="$PWD/pr-view.json"
    nu ../script.nu --mapping "$PWD/mapping.json" fake
    nu -c '
      let d = (open repo/patches/upstream-status.json)
      let p = ($d.patches."0001-fake-fix.patch")
      if $p.pr.state != "merged" or (not $p.retired) {
        error make {msg: $"stage 2: not retired: ($p | to json)"}
      }
      if ($d.log | length) != 3 {
        error make {msg: $"stage 2: expected 3 log transitions, got ($d.log | to json)"}
      }'

    echo "--- stage 3: re-run is idempotent (no duplicate transitions) ---"
    nu ../script.nu --mapping "$PWD/mapping.json" fake
    nu -c '
      let d = (open repo/patches/upstream-status.json)
      if ($d.log | length) != 3 {
        error make {msg: $"stage 3: log grew on a no-change re-run: ($d.log | to json)"}
      }'

    touch "$out"
  '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = (old.passthru.tests or {}) // {inherit lifecycle;};
      };
  })
