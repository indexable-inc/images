# Single source of truth for the de-forked packages: each one pins an upstream
# `flake = false` input and keeps its delta as an ordered `patches/` series
# next to the package (see lib/util/patched-src.nix). One list drives four
# consumers so they cannot drift:
#
#   - `packages/<...>/default.nix` applies the series via `ix.patchedSrc`.
#   - `lib/per-system.nix` exposes each patched source as
#     `checks.<system>.patched-src-<name>` (the seconds-fast conflict gate).
#   - `packages/rebase-patches` reads the rendered JSON (input name, upstream
#     git URL, repo-relative patch dir) to regenerate the series through a real
#     `git rebase` when the pinned base moves.
#   - `packages/upstream-sync` reads the per-patch upstreaming intent and per-repo
#     `upstreamPolicy` to drive the upstreaming loop (refresh tracked PR state,
#     find duplicates, and open PRs for `attempt`-marked patches). See that tool.
#
# Adding a de-forked package is one entry here plus its `patches/` folder.
#
# Fields:
#   name       : package id / patched-src check suffix.
#   input      : flake.lock input name whose `locked.rev` pins the base.
#   url        : upstream git URL the base and rebase fetch from.
#   patchDir   : repo-relative path to the ordered `*.patch` series.
#   autoUpdate : whether the scheduled fork-sync (.github/workflows/fork-sync.yml)
#                may free-float the base under a routine bump. `false` pins the
#                input by rev and keeps it out of the cron; it moves only under a
#                deliberate manual `rebase-patches` run.
#   closureGates : optional, default false. Opt the fork into the
#                per-attempt-patch closure build gates (RFC 0010 A3, #2098):
#                one derivation per attempt-marked patch that rebuilds the
#                fork package with the series restricted to that patch's
#                dag.json closure -- exactly what `upstream-pr` ships
#                upstream, so a red gate means the upstream PR would be
#                broken. Heavy full-package builds, so gates are NEVER flake
#                checks: they surface as `passthru.closureGates` on the fork
#                package and `forkClosureGates.<system>.<name>` on the flake,
#                built by the scheduled fork-closure-gates workflow
#                (post-merge; its static path filters must name this fork's
#                patch dir) and the `upstream-sync --open` preflight. Opting
#                in also requires the package to wire `passthru.closureGates`
#                (see packages/nix/nix/default.nix) and a `gatePackages`
#                entry in lib/per-system.nix (missing one fails eval loudly).
#   forkRepo   : optional GitHub `owner/name` of a real fork repo to maintain.
#                When set, the mirror-sync workflow (packages/mirror,
#                `mirror fork-branch --name <name> --push`) keeps that repo's
#                `ix-patched` branch equal to the pinned base plus this patch
#                series applied as commits, so an upstream PR is one push away.
#                Absent = no fork repo is maintained (the in-repo series stays
#                the only serialization).
#
# Upstreaming intent (hand-written declarative intent; the human gate on the
# outward act). `packages/upstream-sync` reads these; the LIVE state it tracks
# (PR urls, states, retirement) is generated into `upstream-status.json` next to
# each series and is never hand-written.
#
#   upstreamPolicy : per-repo contribution stance, researched from each upstream's
#                    CONTRIBUTING / governance. Fields:
#                      prsWelcome    : does the project accept external PRs at all.
#                      aiPrsAllowed  : true | false | "unknown". Whether AI-generated
#                                      or AI-assisted PRs are permitted. A repo that
#                                      bans them is `never` at the repo level and the
#                                      tool refuses to open any PR against it.
#                      citation      : URL backing `aiPrsAllowed` (the policy doc).
#                      notes         : one line of contribution nuance (CLA, disclosure).
#   patches        : per-patch intent, keyed by the EXACT patch file name (the
#                    stable identity the series and dag.json share; keying by a
#                    derived slug would risk a slug/file mismatch). Each value:
#                      upstream : "attempt" | "hold" | "never".
#                        attempt = we want it upstream and authorize the tool to open
#                                  the PR (the human gate for the outward act).
#                        hold    = wants quality work before it is PR-ready (e.g. the
#                                  clippy lints want a quality pass first).
#                        never   = repo-specific delta or unmergeable upstream; the
#                                  tool never opens a PR for it.
#                      reason   : one line explaining the mark.
#                      prExtra  : OPTIONAL upstream-specific PR-template content
#                                 (issue refs, checklists) that does not belong in
#                                 a commit message; appended after the PR body.
#                    A patch with no entry defaults to `hold` with an "unclassified"
#                    reason (fail-safe: an unclassified patch is never sent upstream
#                    automatically). `upstream-sync` treats a repo whose
#                    `upstreamPolicy.aiPrsAllowed == false` as `never` regardless of
#                    the per-patch mark, so a banned repo cannot leak a PR.
#
# There is deliberately NO per-patch description field: the upstream PR's title
# and body come from the patch's own commit message (subject = title, body = PR
# body, plus AI attribution and a link to the patch of record; see
# packages/upstream-pr). A nix copy would duplicate the commit message and
# drift. One fact, one home: the commit message IS the patch's description and
# its reason of record, and the `patch-dag-<name>` check fails any patch whose
# commit message states no reason (attribution trailers and bare issue refs do
# not count).
{
  forkPackages = [
    {
      # codex is cargoHash-coupled: the package vendors its Cargo dependencies
      # behind a fixed `cargoHash`, and a rebase-patches run does not regenerate
      # that hash, so a free-floating base desyncs the two the moment upstream's
      # Cargo.lock changes. Worse, the desync also hits consumers that lock
      # codex-src transitively (ix), where a routine `nix flake update` floated
      # the base past our hash and broke every prod deploy for 13h on
      # 2026-07-07. The input is pinned by rev in flake.nix; bump it by hand,
      # then `nix run .#rebase-patches -- codex` and regenerate the cargoHash in
      # the same change.
      name = "codex";
      input = "codex-src";
      url = "https://github.com/openai/codex.git";
      patchDir = "packages/agent/codex/patches";
      autoUpdate = false;
      upstreamPolicy = {
        # Codex is invitation-only: unsolicited PRs are closed without review, so
        # `prsWelcome = false` and the tool never opens a PR here regardless of
        # per-patch intent. The AI stance is unstated (the gate is the invitation,
        # not the AI), but it does not matter given prsWelcome = false.
        prsWelcome = false;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/openai/codex/blob/main/docs/contributing.md";
        notes = "Invitation-only: 'does not accept unsolicited code contributions... will be closed without review.' CLA required. External help goes to issues, not PRs.";
      };
      patches = {
        "0001-mcp-route-channel-notifications-into-chat.patch" = {
          upstream = "never";
          reason = "Hard to land upstream (fast-moving OpenAI-owned repo); ix-specific MCP channel-notification routing.";
        };
        "0002-tui-refresh-adaptive-syntax-theme-on-focus-regain.patch" = {
          upstream = "never";
          reason = "General fix for openai/codex#18942, but codex closes unsolicited PRs (prsWelcome = false); the upstream issue is the feedback channel.";
        };
      };
    }
    {
      name = "btop";
      input = "btop-src";
      url = "https://github.com/aristocratos/btop.git";
      patchDir = "packages/terminal/btop/patches";
      autoUpdate = true;
      upstreamPolicy = {
        prsWelcome = true;
        # btop explicitly allows AI-assisted code WITH mandatory disclosure: a PR
        # with any AI-generated code must be tagged `[AI generated]`, and hiding it
        # gets the account blocked. `upstream-sync` attaches AI attribution to every
        # PR body per the outward-message policy, which satisfies this.
        aiPrsAllowed = "true";
        citation = "https://github.com/aristocratos/btop/blob/master/CONTRIBUTING.md";
        notes = "AI code allowed but must be disclosed ([AI generated] tag); undisclosed AI = closed PR / block. Feature PRs: open a feature request first.";
      };
      patches = {
        "0001-Add-macOS-process-disk-IO-sorting.patch" = {
          upstream = "hold";
          reason = "General macOS feature (per-process disk IO sorting) plausibly welcome upstream, but wants a quality pass and a discussion issue first per btop CONTRIBUTING.";
        };
        "0002-proc-show-kernel-working-directory-cwd-in-the-detail.patch" = {
          upstream = "hold";
          reason = "General feature (show process cwd in detail view); wants a quality pass and a discussion issue first.";
        };
      };
    }
    {
      # clippy is nightly-toolchain-coupled: its input is pinned by rev and must
      # move only with the pinned nightly, so `rebase-patches` is run explicitly
      # alongside a toolchain bump, never under a blanket `nix flake update` or
      # the scheduled fork-sync. `name` is `clippy` (not the `llm-clippy` package
      # id) so the check reads `patched-src-clippy` and the rebase arg is `clippy`.
      name = "clippy";
      input = "clippy-src";
      url = "https://github.com/rust-lang/rust-clippy.git";
      patchDir = "packages/llm-clippy/patches";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        # Clippy inherits rust-lang/rust's binding LLM policy (rust-forge#1040). It
        # permits LLMs to analyze/review/refine but BANS LLM-*created* code, comments,
        # docs, and diagnostics except under reviewer-solicited "experiment rules".
        # A new lint is exactly LLM-created code + diagnostics, so autonomous PR
        # creation is NOT allowed here: `aiPrsAllowed = false`, which makes the tool
        # refuse to open any clippy PR at the repo level (defense in depth on top of
        # the per-patch `hold`). Landing a lint upstream is a human-driven,
        # reviewer-solicited effort, not an agentic outward act.
        aiPrsAllowed = "false";
        citation = "https://github.com/rust-lang/rust-forge/blob/master/src/policies/llm-usage.md";
        notes = "rust LLM policy: fine to analyze/review, NOT to create code/comments/docs/diagnostics except under experiment rules. New lints also need a proposal issue + discussion. The clippy quality pass is a human-driven follow-up.";
      };
      patches = {
        # The nightly-sync commit is our rebase mechanism onto the pinned
        # toolchain; it is meaningless upstream.
        "0001-Update-Clippy-for-nightly-2026-05-27-repo-toolchain-.patch" = {
          upstream = "never";
          reason = "Repo-specific: pins clippy to our nightly toolchain; not an upstream change.";
        };
        # The ten new lints: the user's default is `attempt` but HOLD for a
        # quality pass (a lint upstream needs a proposal issue, docs, ui tests,
        # and a stabilization discussion). The clippy quality pass is a follow-up.
        "0002-Add-module_file_count-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate but wants a quality pass (lint proposal issue, docs, ui tests) before a PR.";
        };
        "0003-Add-excessive_file_length-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0004-Add-path_segment_repetition-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0005-Add-underscore_in_module_filename-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0006-Add-renamed_imports-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0007-Add-fallible_int_fallback-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0008-Add-magic_number-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0009-Add-drop_must_use-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0010-Add-non_trait_imports-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        # Adds ix-specific package metadata to clippy's own Cargo manifests.
        "0011-Add-ix-metadata-to-Cargo-manifests.patch" = {
          upstream = "never";
          reason = "Repo-specific: adds ix packaging metadata to clippy's Cargo manifests.";
        };
        "0012-Add-string_ip_field-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        # Tracks Cargo.lock so our nix consumers get a stable lock; upstream
        # clippy deliberately gitignores it.
        "0013-track-Cargo.lock-so-downstream-nix-consumers-don-t-c.patch" = {
          upstream = "never";
          reason = "Repo-specific: tracks Cargo.lock for our nix consumers; upstream intentionally gitignores it.";
        };
        "0014-Add-anonymous-tuple-return-type-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
      };
    }
    {
      # mesa is panes-GPU-coupled: its input is pinned by rev (upstream tag
      # mesa-26.1.2) and must move only under a deliberate bump, never a blanket
      # `nix flake update` or the scheduled fork-sync. The venus driver-side
      # sync-fd patch (index#1742) is validated by BOOTING the panes guest on a
      # linux GPU host and exercising the WSI acquire path, not by CI, so a base
      # bump is a rebase-plus-boot event, not a routine cron. `url` is the
      # gitlab git remote so `rebase-patches`' scratch-clone fetch works; the
      # build consumes `ix.mesaSrc` (the shallow git input) through patchedSrc.
      name = "mesa";
      input = "mesa-src";
      url = "https://gitlab.freedesktop.org/mesa/mesa.git";
      patchDir = "packages/vm/panes/guest-image/mesa/patches";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        # mesa is on GitLab, not GitHub: the gh-based PR path does not apply.
        # `upstream-sync` cannot open a mesa MR (it is a GitHub tool), so every
        # mesa patch is effectively never for the automated path; contribution is
        # via a freedesktop.org GitLab merge request by hand.
        # Mesa allows AI-assisted code with mandatory Assisted-by/Generated-by
        # trailers, but BANS fully autonomous-agent submissions: a human must open
        # and drive the MR. Combined with the GitLab-not-GitHub gap, the automated
        # path is off regardless; mesa patches are contributed by hand.
        aiPrsAllowed = "false";
        citation = "https://docs.mesa3d.org/submittingpatches.html";
        notes = "GitLab MR workflow, not GitHub; upstream-sync's gh path cannot open a mesa MR. Also bans autonomous-agent submissions (human must drive the MR). Contribute by hand via gitlab.freedesktop.org with Assisted-by/Generated-by trailers.";
      };
      patches = {
        "0001-venus-handle-temporary-sync-fd-semaphore-imports-dri.patch" = {
          upstream = "never";
          reason = "Real venus driver fix and a strong upstream candidate, but mesa is GitLab: upstream-sync's gh path cannot open the MR. Contribute by hand.";
        };
        "0002-README.ix-document-snapshot-fork-layout.patch" = {
          upstream = "never";
          reason = "Repo-specific: documents our snapshot fork layout; meaningless upstream.";
        };
        "0003-venus-fail-sparse-batches-waiting-on-driver-side-syn.patch" = {
          upstream = "never";
          reason = "Real venus driver fix but mesa is GitLab; upstream-sync's gh path cannot open the MR. Contribute by hand.";
        };
      };
    }
    {
      # nix is our daemon toolchain: the base is the exact rev the hydra daemon
      # runs (tag 2.34.7), so the patched package is a protocol-compatible
      # drop-in for the running daemon. The base moves DELIBERATELY, in the same
      # change that moves the daemon version, never under a routine
      # `nix flake update` or the scheduled fork-sync -- hence `autoUpdate =
      # false`, which pins `nix-src` by rev and keeps it out of the cron. Bump the
      # `nix-src` rev, then `nix run .#rebase-patches -- nix`.
      name = "nix";
      input = "nix-src";
      url = "https://github.com/NixOS/nix.git";
      patchDir = "packages/nix/nix/patches";
      autoUpdate = false;
      # nix is the one fork whose attempt patches ship upstream as standalone
      # dag.json closures, so it pays for the per-attempt closure build gates
      # (RFC 0010 A3): 9 attempt patches = 9 scheduled full-package builds,
      # cache hits between changes. See the `closureGates` field doc above.
      closureGates = true;
      upstreamPolicy = {
        prsWelcome = true;
        # NixOS/nix now has an explicit AI/automation policy (NixOS/nix#15984,
        # adapted from nixpkgs' with EXTRA constraints on human communication).
        # Its three operative constraints all cut against an agent opening PRs
        # here: (1) HUMAN COMMUNICATION -- a responsible human in the loop must
        # author the PR text and comments (hallucinated slop comments were the
        # motivating harm); (2) NO UNREVIEWED AUTOMATED SUBMISSIONS -- an agent
        # may not file the PR itself; a human reviews and submits; (3) ASSISTED-BY
        # DISCLOSURE -- AI-assisted work must be disclosed with an `Assisted-by:`
        # commit trailer. So `aiPrsAllowed = false`: the tool refuses to open ANY
        # nix PR at the repo level (defense in depth on top of the per-patch
        # `hold`). Contribution here is a human-driven act -- Andrew submits, the
        # patches carry `Assisted-by` trailers, and the tool only ever plans and
        # tracks, never opens.
        aiPrsAllowed = "false";
        citation = "https://github.com/NixOS/nix/pull/15984";
        notes = "AI policy (#15984): human must author PR communication, no unreviewed automated submissions, disclose AI assistance with an Assisted-by trailer. Agent-filed PRs are out; a human submits with the patches' Assisted-by trailers.";
      };
      # All nix patches are HOLD: the repo-level `aiPrsAllowed = false` (see the
      # policy above) already blocks the outward act, and the per-patch marks
      # record the human follow-up each needs so nothing reads as agent-ready.
      # The commit-message body is still the source of truth for each PR; the
      # human handoff kit (drafts + submission plan) lives outside nix.
      patches = {
        # 0001: reworked to the `catch (BaseError&)` shape (widen the existing
        # handler rather than a blanket `catch (...)`), the narrowing Andrew
        # proposed in the #15963 discussion after xokdvium objected to swallowing
        # all exceptions. Ready in shape but a human (Andrew) reopens/submits it.
        "0001-fix-libstore-don-t-crash-the-daemon-when-a-GC-roots-.patch" = {
          upstream = "hold";
          reason = "Reworked to catch (BaseError&) per the #15963 review (xokdvium: `catch (...)` swallows too much); a human (Andrew) resubmits, referencing #15963/#15962/#13438. Fixes NixOS/nix#15962.";
        };
        # 0002: the cleanest single-file candidate -- a regression restoration.
        # #8240 made nix's default-path probing EPERM/EACCES-tolerant on the
        # macOS sandbox (treat permission-denied like absent); the later
        # std::filesystem migration reintroduced the throwing exists() overload
        # that #5884 first flagged and #8485 still tracks. A human submits it
        # framed as restoring that lost behavior.
        "0002-fix-libexpr-treat-inaccessible-default-lookup-path-e.patch" = {
          upstream = "hold";
          reason = "Cleanest candidate: restores the EPERM-tolerant default-path probing of #8240 lost in the std::filesystem migration (see #5884, still-open #8485). Human submits, framed as a regression fix.";
        };
        # The build-status directory series (0003-0009): DO NOT file a competing
        # series. edolstra's active #15979 (`nix ps`) covers the same
        # build-observability ground from the live process-tree side. Engage
        # THERE with our complementary daemon-less, file-based angle (honors
        # NIX_STATE_DIR, works when the daemon is wedged / the store lock is
        # contended -- exactly where `nix ps` hangs) rather than opening a rival
        # PR. Held pending that conversation.
        "0003-libutil-add-build-status-dir-experimental-feature.patch" = {
          upstream = "hold";
          reason = "Build-status series overlaps edolstra's active #15979 (nix ps); engage there with the daemon-less file-based angle instead of filing a competing series.";
        };
        "0004-libstore-add-build-status-directory-writer.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "0005-libstore-write-status-files-from-build-and-substitut.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "0006-libstore-daemon-record-client-uid-and-user-for-build.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "0007-nix-add-nix-store-builds-command.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "0008-tests-functional-test-build-status-directory.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "0009-doc-release-note-for-build-status-directory-and-nix-.patch" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        # Structured git history export (RFC 0011). Designed to be
        # upstreamable (deterministic, opt-in, experimental-feature gated,
        # never in lock files -- it dodges the objections that sank
        # leaveDotGit-for-flakes), but held: repo-wide upstreaming pause
        # (NixOS/nix#15984, see #2021), and a feature of this size should
        # start as an upstream discussion, not a cold PR.
        "0010-libfetchers-add-opt-in-structured-commit-history-exp.patch" = {
          upstream = "hold";
          reason = "Feature-sized change; upstreaming paused per NixOS/nix#15984 and it should open as an upstream issue/RFC first.";
        };
        # 0011: temp roots for in-flight CA build outputs, closing the min-free
        # auto-GC race that broke wide cargo-unit graphs (index#2334).
        "0011-fix-libstore-add-temp-roots-for-CA-derivation-output.patch" = {
          upstream = "hold";
          reason = "Fix for min-free auto-GC deleting in-flight CA build outputs (indexable-inc/index#2334). Hold: humans submit nix patches upstream per NixOS/nix#15984; overlaps the still-open upstream discussion NixOS/nix#15613 / NixOS/nix#15719.";
        };
      };
    }
  ];
}
