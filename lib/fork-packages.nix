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
#   derivedPatches : optional list of patches DERIVED BY NIX at build time
#                instead of stored as line diffs -- for mechanical or generated
#                content (a stanza stamped into every manifest, a tracked
#                lockfile) that re-conflicts on every rebase when kept as a
#                diff. Each entry:
#                  name      : short id (names the generated patch derivation).
#                  generator : repo-relative path to a .nix file evaluating to
#                              `{pkgs, src, ...}: drv` whose output is a single
#                              unified-diff file produced at BUILD time from
#                              the actual pinned tree (copy src, apply the
#                              mechanical edit, diff old new). A generator must
#                              fail loudly behind a structural guard (e.g.
#                              "every `[package]` manifest got the stanza,
#                              count > 0"), never silently no-op, and never
#                              bake in magic totals that go stale.
#                  reason    : one line, the patch's reason of record. Derived
#                              patches are not commits, so the dag commit-body
#                              reason check cannot cover them; this field does.
#                  upstream  : always "never". A derived patch is repo-local
#                              mechanical output and is invisible to
#                              rebase-patches, dag.json, and upstream-sync by
#                              construction (it is not a `*.patch` file), so it
#                              can never be rebased, dag-tracked, or sent
#                              upstream.
#                `patchedSrc` appends the generator outputs after the static
#                series; see lib/util/patched-src.nix.
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
#                    automatically) -- but for a fork that declares any intent, the
#                    `patch-dag-<name>` check fails a patch with no entry, so the
#                    fallback only backstops forks with no `patches` attrset at all.
#                    `upstream-sync` treats a repo whose
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
      # Codex uses importCargoLock, but git dependencies still carry fixed
      # output hashes in packages/agent/codex/default.nix. A free-floating base
      # can move Cargo.lock past those hashes, including for downstream flakes
      # that lock codex-src transitively; that broke every ix prod deploy for
      # 13h on 2026-07-07. The input is pinned by rev in flake.nix. Bump it by
      # hand, rebase the patches, then build Codex and refresh any git dependency
      # hashes named by Nix.
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
      name = "zed";
      input = "zed-upstream";
      url = "https://github.com/zed-industries/zed.git";
      patchDir = "packages/zed/patches";
      autoUpdate = false;
      forkRepo = "indexable-inc/zed";
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "false";
        citation = "https://github.com/zed-industries/zed/blob/main/CONTRIBUTING.md#ai-policy";
        notes = "Zed permits human-directed LLM assistance but rejects autonomous-agent contributions; keep this patch in the maintained fork unless a human takes it upstream.";
      };
      patches = {
        "0001-editor-optionally-exclude-the-invocation-reference.patch" = {
          upstream = "never";
          reason = "Useful general editor behavior, but Zed's contribution policy rejects autonomous-agent submissions.";
        };
        "0002-nix-expose-stable-application-package.patch" = {
          upstream = "never";
          reason = "Required to install the stable app from Zed's own flake, but Zed's contribution policy rejects autonomous-agent submissions.";
        };
        "0003-editor-navigate-directly-to-a-single-reference.patch" = {
          upstream = "never";
          reason = "General editor behavior fix (indexable-inc/index#2976), but Zed's contribution policy rejects autonomous-agent submissions.";
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
      # Home Manager is consumed as a FLAKE by workstation config repos, not
      # as a package built here, so the series' consumer is the maintained
      # fork repo: `mirror fork-branch --name home-manager --push` keeps
      # indexable-inc/home-manager's `ix-patched` branch equal to the pinned
      # base plus this series, and a config repo points its `home-manager`
      # flake input at that branch. Pinned by rev (autoUpdate = false): there
      # is no `.#home-manager.updateScript` package for the fork-sync cron to
      # drive; bump by hand with `nix flake update home-manager-src` + `nix
      # run .#rebase-patches -- home-manager`, then re-push the fork branch.
      name = "home-manager";
      input = "home-manager-src";
      url = "https://github.com/nix-community/home-manager.git";
      patchDir = "packages/home-manager/patches";
      autoUpdate = false;
      forkRepo = "indexable-inc/home-manager";
      upstreamPolicy = {
        prsWelcome = true;
        # The contributing manual (guidelines, getting-started) has no
        # AI-contribution policy as of 2026-07-19; PRs are welcome with
        # commit-format and test expectations spelled out there.
        aiPrsAllowed = "unknown";
        citation = "https://nix-community.github.io/home-manager/#ch-contributing";
        notes = "PRs welcome. Commit subject `{module}: summary` <= 50 chars, body explains motivation; changes should carry tests. No CLA.";
      };
      patches = {
        "0001-files-batch-symlink-creation-and-target-checks-in-ac.patch" = {
          upstream = "attempt";
          reason = "General activation performance fix (per-file fork+exec dominates linkGeneration/checkLinkTargets on darwin, 3.4s -> 0.2s for 365 links); behavior-preserving and upstream-quality.";
        };
      };
    }
    {
      name = "git";
      input = "git-src";
      url = "https://github.com/git/git.git";
      patchDir = "packages/git/patches";
      # The package overlays nixpkgs' git recipe onto this source, so the base
      # must equal nixpkgs' git version tag (v2.54.0), never free-float under
      # the fork-sync cron. Repin manually when nixpkgs bumps git.
      autoUpdate = false;
      upstreamPolicy = {
        # git/git on GitHub is a read-only mirror: contributions go through the
        # mailing list (or GitGitGadget), never GitHub PRs, so the tool must not
        # open one. Upstreaming this series is a manual mailing-list submission.
        prsWelcome = false;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/git/git/blob/master/Documentation/SubmittingPatches.adoc";
        notes = "Mailing-list workflow (git@vger.kernel.org / GitGitGadget); DCO sign-off required; no AI-specific policy found as of 2026-07-18.";
      };
      patches = {
        "0001-submodule-helper-borrow-common-dir-module-store-in-l.patch" = {
          upstream = "hold";
          reason = "General fix git upstream anticipated in df56607dff2 but never implemented; wants a mailing-list submission with review, which upstream-sync cannot automate for a non-PR project.";
        };
      };
    }
    {
      name = "nushell";
      input = "nushell-src";
      url = "https://github.com/nushell/nushell.git";
      patchDir = "packages/nushell/patches";
      autoUpdate = true;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nushell/nushell/blob/main/CONTRIBUTING.md";
        notes = "PRs welcome for focused changes; CONTRIBUTING has no AI-specific policy as of 2026-07-07. Include tests and user-facing release-note context.";
      };
      patches = {
        "0001-Add-xattrs-column-to-ls-l.patch" = {
          upstream = "attempt";
          reason = "General filesystem feature requested in nushell/nushell#7106; prior PR #7158 was abandoned and explicitly left open for takeover.";
          prExtra = "Related issue: nushell/nushell#7106. Prior closed attempt: nushell/nushell#7158.";
        };
        "0002-Derive-feature-list-for-cargo-unit-builds.patch" = {
          upstream = "never";
          reason = "Repo-specific: cargo-unit does not export Cargo's aggregate CARGO_CFG_FEATURE env var, so the package derives it from CARGO_FEATURE_* for ix builds.";
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
        "0011-Add-string_ip_field-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "0012-Add-anonymous-tuple-return-type-lint.patch" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
      };
      # Mechanical deltas derived by nix from the pinned tree (see the
      # `derivedPatches` field doc above): stored line diffs for these
      # re-conflicted on every rebase and went stale whenever upstream added a
      # manifest or moved a dependency.
      derivedPatches = [
        {
          name = "ix-metadata";
          generator = "packages/llm-clippy/patches/derived/ix-metadata.nix";
          reason = "Repo-specific: stamps [package.metadata.ix.inputs] into every [package] manifest so cargo-unit treats the fork's crates like repo crates; derived from the tree so new upstream manifests are covered automatically.";
          upstream = "never";
        }
        {
          name = "cargo-lock";
          generator = "packages/llm-clippy/patches/derived/cargo-lock.nix";
          reason = "Repo-specific: tracks Cargo.lock for our nix consumers (upstream intentionally gitignores it); the committed lockfile lives next to the generator and moves only with the nightly / clippy-src bump.";
          upstream = "never";
        }
      ];
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
        "0006-libstore-daemon-record-client-identity-for-build.patch" = {
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
        # 0012: temp root for the floating-CA scratch output path itself
        # (makeFallbackPath), the residual GC window 0011 left open: a
        # non-chroot builder writes the unregistered scratch path directly,
        # and a concurrent GC deletes it mid-build (index#2354).
        "0012-fix-libstore-add-temp-root-for-floating-CA-scratch-o.patch" = {
          upstream = "hold";
          reason = "Companion to 0011: roots the floating-CA scratch output path during non-chroot builds (indexable-inc/index#2354). Upstream master has the same gap, but humans submit nix patches upstream per NixOS/nix#15984.";
        };
        # 0013: opt-in `forge-fetch-via-git` -- fetch github:/gitlab:/sourcehut:
        # inputs through the Git smart protocol into the tarball cache (delta
        # transfers via a per-repo negotiation ref) instead of downloading a
        # full archive of every new revision. Bit-identical to the archive path
        # (archive-compatible-tree check with automatic tarball fallback), so
        # upstreamable in principle, but held like 0010: feature-sized fetcher
        # changes should start as an upstream discussion, not a cold PR.
        "0013-libfetchers-opt-in-incremental-fetching-of-forge-inp.patch" = {
          upstream = "hold";
          reason = "Feature-sized fetcher change; upstreaming paused per NixOS/nix#15984 and it should open as an upstream issue/discussion first (touches lock-file-adjacent fetch semantics).";
        };
        # 0014: underscore digit separators in numeric literals (`1_000`,
        # `1_000.000_1`, `2.5e1_0`), Rust-shaped (between digits only; a
        # leading underscore is still an identifier), stripped before the
        # value is parsed. Fork-only syntax is allowed only inside import
        # islands wrapped in `ix.evaluatorGate.require` (today: tests/);
        # everything else stays stock-parseable so external flake consumers
        # and the `nix-ix` bootstrap keep evaluating on upstream Nix,
        # enforced by `checks.<system>.stock-nix-parse` (index#3635).
        # astlog's digit-grouping lints track the remaining toolchain
        # backlog (astlog-rules/nix.astlog).
        "0014-libexpr-accept-underscore-digit-separators-in-numeri.patch" = {
          upstream = "hold";
          reason = "Language syntax change; must start as an upstream issue/RFC, and humans submit nix patches upstream per NixOS/nix#15984.";
        };
        "0015-fix-libcmd-preserve-repeated-installable-cardinality.patch" = {
          upstream = "hold";
          reason = "Fixes repeated installables multiplying `nix build --json` results (indexable-inc/index#2633). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0016: a newer Nix uses opaque per-instance temporary-root filenames.
        # The 2.34 collector parsed every entry as a decimal PID, so one newer
        # file disabled both scheduled and reactive GC until the store filled
        # (index#3031). Upstream master already treats the name as opaque in
        # NixOS/nix#15992; this is the reader-side backport for mixed versions.
        "0016-fix-libstore-accept-opaque-temporary-root-filenames.patch" = {
          upstream = "hold";
          reason = "Backports the mixed-version temporary-root reader from NixOS/nix#15992 (indexable-inc/index#3031). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0017: each daemon process decides whether to auto-GC before waiting
        # for the store-global gc.lock. Recheck under that lock so queued
        # callers do not repeat a collection after the first restores space.
        "0017-fix-libstore-recheck-free-space-after-GC-lock.patch" = {
          upstream = "hold";
          reason = "Prevents stale queued auto-GC decisions from serializing CI jobs behind repeated collections (indexable-inc/index#3085, indexable-inc/ix#7145). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0018: a daemon worker loses its signal thread at fork while retaining
        # the blocked mask, then synchronous auto-GC can wait forever on a
        # detached collector queued at gc.lock after the client disappears.
        "0018-fix-libstore-interrupt-blocked-automatic-GC.patch" = {
          upstream = "hold";
          reason = "Restarts forked daemon signal handling and makes blocked auto-GC observe cancellation (indexable-inc/index#3300, indexable-inc/ix#7145). Signal handling ports Lix dccde9436; humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        "0019-Get-rid-of-duplicated-Build-failed-due-to-failed-dep.patch" = {
          upstream = "hold";
          reason = "Backports NixOS/nix#16040 from 2.34.8; drop when the daemon base reaches 2.34.8 or newer.";
        };
        "0020-libstore-preserve-content-addressed-leaf-failures.patch" = {
          upstream = "hold";
          reason = "Preserves actionable floating-CA leaf failures through resolution (indexable-inc/index#3279, indexable-inc/ix#7357). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0021: CPU, I/O, and build-log activity distinguish a silent active
        # compiler from an idle builder before the daemon cancels its goal.
        "0021-libstore-enforce-derivation-no-progress-deadlines.patch" = {
          upstream = "hold";
          reason = "Fleet CI policy for indexable-inc/index#3317. Validate the process-aware deadline before proposing a general Nix interface; humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0022: a paused (backpressured) substitution download neither reads
        # its socket nor advances CURLOPT_LOW_SPEED_TIME, so a peer half-close
        # parked the transfer forever -- nix-daemon children stranded in
        # CLOSE-WAIT wedged CI slots for hours. The worker loop now polls
        # paused transfers and fails them as transient curl errors, so
        # download-attempts still applies.
        "0022-libstore-fail-paused-downloads-on-peer-half-close-or.patch" = {
          upstream = "hold";
          reason = "Fails paused downloads on peer half-close or stall past stalled-download-timeout instead of parking forever (indexable-inc/index#3559). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0023: forge archive inputs (github:/gitlab:/sourcehut:) requesting
        # submodules or LFS are constructed as the equivalent git+https input
        # (archive tarballs cannot contain that data), so lock files record a
        # plain `git` node stock Nix understands. Fixes the hard failure of
        # NixOS/nix#13571 and the silent empty-submodule trees of
        # NixOS/nix#14982; the mapping mirrors GitHubInputScheme::clone().
        "0023-libfetchers-fetch-forge-inputs-via-git-when-submodul.patch" = {
          upstream = "hold";
          reason = "Implements roberth's implicit git+https switch from NixOS/nix#14982 (also fixes NixOS/nix#13571; indexable-inc/index#3626). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0024: the child's own flake.lock is authoritative for the subtree of
        # a relative path flake input (the child lives in the parent's tree,
        # so its fetch is free and its content is already pinned). Scoped
        # first step of roberth's approved sparseNodes plan (NixOS/nix#7730,
        # Nov 2025, unimplemented upstream); lock format untouched, so the
        # patch drops cleanly when upstream ships the real migration. Also
        # carries the kept-flake relative-input refetch of the open upstream
        # PR NixOS/nix#15982 (bug NixOS/nix#14762), which the deferral needs
        # for nested relative inputs. Extends the merged update-time child
        # lock respect of NixOS/nix#13437 to every lock computation.
        "0024-libflake-defer-to-the-child-lock-for-relative-path-f.patch" = {
          upstream = "hold";
          reason = "Eval/lock-time sparse lock semantics for relative path inputs, the scoped start of the sparseNodes plan (NixOS/nix#7730; indexable-inc/index#3627). Hold: humans submit Nix patches upstream per NixOS/nix#15984, and the sparseNodes migration should land via the upstream plan, not a cold fork PR.";
        };
        # 0025-0028: lazy trees (indexable-inc/index#3645), vendored as the
        # variant upstream actually merged to master (NixOS/nix#15711 plus its
        # two post-merge fixes), not the closed lazy-trees-v2 PR
        # (NixOS/nix#13225) and not Determinate's random-virtual-path
        # implementation: roberth rejected randomness/fingerprint placeholders
        # as impure (path equality and ordering must stay observably
        # identical; his rope-string lazy-hashing alternative is unbuilt), and
        # #13225 was closed 2026-07-16 pointing at #15711 as the mergeable
        # subset. Determinate's own v2.35.1 tree has dropped the
        # random-path implementation and rides the same mechanism. All four
        # are already on upstream master; drop when the base reaches 2.35+.
        "0025-libexpr-Add-a-way-to-collect-string-context-from-Val.patch" = {
          upstream = "hold";
          reason = "Backports upstream 569ee752c (prerequisite of NixOS/nix#15711, merged to master 2026-04-27); drop when the base reaches a release containing it (2.35+).";
        };
        "0026-Don-t-copy-flakes-to-the-store-unnecessarily.patch" = {
          upstream = "hold";
          reason = "Backports upstream 891ef140b (NixOS/nix#15711, merged to master 2026-04-27); drop when the base reaches a release containing it (2.35+).";
        };
        "0027-libexpr-Make-hash-mismatches-while-copying-lazy-path.patch" = {
          upstream = "hold";
          reason = "Backports upstream 8ffda0826 (NixOS/nix#15950, post-merge fix to #15711); drop when the base reaches a release containing it (2.35+).";
        };
        "0028-libexpr-Handle-lazy-paths-in-builtins.storePath-bett.patch" = {
          upstream = "hold";
          reason = "Backports upstream 933f3140b (NixOS/nix#16078, post-merge fix to #15711); drop when the base reaches a release containing it (2.35+).";
        };
        # 0029: our divergence from upstream master, which enables lazy
        # mounting unconditionally. The off-by-default `lazy-trees` setting
        # keeps the fork byte-identical to eager evaluation unless opted in;
        # flipping the fleet on is a separate decision with its own drv-hash
        # and eval-result equivalence sweeps (indexable-inc/index#3645).
        "0029-libexpr-gate-lazy-input-mounting-behind-an-off-by-de.patch" = {
          upstream = "hold";
          reason = "Fork policy gate for the 0025-0028 backports (indexable-inc/index#3645): upstream ships the behavior unconditionally, we default it off pending fleet-wide equivalence sweeps. Retire together with the backports when the base reaches 2.35+.";
        };
        # 0030: a relative path input that is a git submodule of its parent
        # is a pinned tree, but its lock node carried no metadata; consumers
        # could not see the pin's age or provenance (a prompt segment showed
        # a fresh pin as 20653 days old, indexable-inc/index#3733). Stamps
        # the gitlink rev and its commit time into the locked ref, so
        # flake.lock and inputs.<name>.{lastModified,rev,shortRev} carry
        # them. Plain subdirectories stay unstamped: their time equals the
        # parent's and would churn the lock on every parent commit.
        "0030-libflake-stamp-submodule-metadata-on-relative-path-f.patch" = {
          upstream = "hold";
          reason = "Submodule metadata for relative path inputs (indexable-inc/index#3737); companion to 0024 in the sparseNodes direction (NixOS/nix#7730). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0031: every GitRepoImpl::fetch targets a nix-internal cache repo
        # (~/.cache/nix/gitv3/*, tarball-cache-v2) that inherits the user's
        # global gitconfig. With maintenance.auto=true + gc.autodetach=true
        # there, each unguarded fetch spawned a detached `git maintenance
        # run --auto` that SIGSEGVs on macOS (gettext locale init ->
        # CFLocaleCopyPreferredLanguages -> NULL CF distributed-notification
        # center -> PAC check at 0x8), so cache maintenance never completed
        # (tarball-cache-v2: 3700 packs / 932MB). Extends the guard patch
        # 0013 already applied to packfilesOnly fetches to every cache-repo
        # fetch.
        "0031-fix-libfetchers-keep-user-git-auto-maintenance-out-o.patch" = {
          upstream = "hold";
          reason = "Keeps user git auto-maintenance out of nix-internal cache repos; fetch-spawned detached `git maintenance run --auto` SIGSEGVs on macOS (indexable-inc/index#3755). Upstream-nix candidate, held: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # With lazy-trees on, path inputs and dirty git worktrees are read
        # from the live filesystem for the whole eval, so a concurrent
        # writer kills long evals with "contents have changed" (exit 102).
        # Snapshot such trees at mount time via clonefile(2) (~70ms) and
        # evaluate from the snapshot; fall back to an eager copy where
        # cloning is unavailable.
        "0032-libexpr-snapshot-mutable-source-trees-at-mount-time-.patch" = {
          upstream = "hold";
          reason = "Fixes the lazy-trees mid-eval mutation race for mutable local trees (indexable-inc/index#3749). Upstream-nix candidate once lazy trees settle there, held: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0033: a daemon worker whose client dies can sleep forever in
        # waitForInput's poll (interrupt delivery is edge-triggered; a
        # trigger landing between the last checkInterrupt and the poll
        # syscall is a lost wakeup), surviving its client for hours while
        # holding goals, builders, and locks (indexable-inc/index#3752).
        # Upstream master has the same gap: its Waker self-pipe only serves
        # cross-thread goal wakeups and is not wired to triggerInterrupt.
        "0033-libstore-wake-the-goal-loop-when-the-process-is-inte.patch" = {
          upstream = "hold";
          reason = "Level-triggered interrupt wakeup for the goal loop (indexable-inc/index#3752); upstream master's Waker pipe is not interrupt-wired, so the bug exists there too. Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0034: `nix store builds` pruned dead writers with kill(pid, 0),
        # which still succeeds for zombies -- 33 phantom "in flight" builds
        # owned by three zombie workers survived 10.5h on hydra
        # (indexable-inc/index#3752). Writers now hold a lifetime flock on
        # their entry; readers treat an acquirable lock (or, for legacy
        # entries, a dead-or-zombie pid) as proof the writer is gone.
        "0034-libstore-prove-build-status-writers-alive-with-a-lif.patch" = {
          upstream = "hold";
          reason = "Build-status series follow-up (zombie-proof staleness, indexable-inc/index#3752): engage on #15979 rather than open a competing PR.";
        };
      };
    }
    {
      # nix-fast-build is the CI build engine (the `check` app): the package
      # overlays this patched source onto nixpkgs' nix-fast-build recipe
      # (packages/nix/nix-fast-build), so the base must equal the nixpkgs
      # package version (tag 1.6.0), never free-float under the fork-sync
      # cron. On a nixpkgs nix-fast-build bump, repin the input to the
      # matching tag and run `nix run .#rebase-patches -- nix-fast-build`.
      name = "nix-fast-build";
      input = "nix-fast-build-src";
      url = "https://github.com/Mic92/nix-fast-build.git";
      patchDir = "packages/nix/nix-fast-build/patches";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/Mic92/nix-fast-build";
        notes = "No CONTRIBUTING or AI policy published as of 2026-07-19; small focused PRs with tests are the observed norm.";
      };
      patches = {
        "0001-workers-make-skip-cached-skip-locally-realized-outpu.patch" = {
          upstream = "hold";
          reason = "Changes what --skip-cached means for `local` outputs for every user; upstream would plausibly want it opt-in, so it needs reshaping as a flag before a PR.";
        };
        "0002-build-add-a-typed-per-derivation-no-progress-deadlin.patch" = {
          upstream = "never";
          reason = "Depends on index's nix fork (build-status directory, patches 0003-0009/0021) that upstream Nix does not have; unmergeable until that daemon interface exists upstream.";
        };
      };
    }
    {
      # nix-derivation is the Haskell .drv parser nix-output-monitor links;
      # packages/nix/nix-output-monitor feeds this patched source into a
      # haskellPackages.extend override. The base is upstream main while its
      # cabal version still reads 1.1.3 (the hackage release nixpkgs builds,
      # plus the bound-relaxation cabal revisions hackage layers on top), so
      # it must not free-float: repin when nixpkgs moves past 1.1.3, then
      # `nix run .#rebase-patches -- nix-derivation`.
      name = "nix-derivation";
      input = "nix-derivation-src";
      url = "https://github.com/Gabriella439/Haskell-Nix-Derivation-Library.git";
      patchDir = "packages/nix/nix-output-monitor/patches";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/Gabriella439/Haskell-Nix-Derivation-Library";
        notes = "No CONTRIBUTING or AI policy; the CA gap is tracked upstream as issue #28 with PR #26 proposing a sum-type DerivationOutput.";
      };
      patches = {
        "0001-Parser-accept-empty-output-paths-in-floating-CA-deri.patch" = {
          upstream = "hold";
          reason = "Upstream PR #26 already proposes the larger sum-type fix for issue #28; ours is the deliberately smaller parser widening, so engage on #26/#28 rather than file a competing PR.";
        };
      };
    }
    {
      # rnix (nix-community/rnix-parser) is patched as a *vendored cargo
      # crate*, not as a package source: lib/util/rnix-digit-separators
      # rewrites the crate inside each consuming tool's cargo vendor dir at
      # build time, selecting the series matching the vendored version
      # (0.11/0.12 share one tokenizer shape, 0.13/0.14 the other). These two
      # entries pin the upstream tags the in-use crates were cut from --
      # v0.12.0 (alejandra, deadnix) here, v0.14.0 (statix) below -- so each
      # series gets the standard canonical form, patched-src apply gate, and
      # dag/reason checks; the build-time patcher reads the same patch files
      # from these patchDirs. Repin alongside the vendored-version change on a
      # nixpkgs bump (the build fails loudly on an unknown rnix version).
      name = "rnix-0-12";
      input = "rnix-0-12-src";
      url = "https://github.com/nix-community/rnix-parser.git";
      patchDir = "lib/util/rnix-digit-separators/patches-0.12";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nix-community/rnix-parser";
        notes = "nix-community project, PRs welcome, no stated AI policy; the patched dialect is gated on the Nix language itself changing, and 0.12 is a historical tag that upstream would not amend anyway.";
      };
      patches = {
        "0001-tokenizer-accept-underscore-digit-separators-in-nume.patch" = {
          upstream = "hold";
          reason = "Lexes a dialect only index's patched nix accepts (packages/nix/nix/patches/0014); upstream rnix should not take it before the Nix language change lands upstream.";
        };
      };
    }
    {
      # See the rnix-0-12 entry above: same logical patch on the v0.14.0
      # tokenizer generation (statix's vendored rnix).
      name = "rnix-0-14";
      input = "rnix-0-14-src";
      url = "https://github.com/nix-community/rnix-parser.git";
      patchDir = "lib/util/rnix-digit-separators/patches-0.14";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nix-community/rnix-parser";
        notes = "nix-community project, PRs welcome, no stated AI policy; the patched dialect is gated on the Nix language itself changing.";
      };
      patches = {
        "0001-tokenizer-accept-underscore-digit-separators-in-nume.patch" = {
          upstream = "hold";
          reason = "Lexes a dialect only index's patched nix accepts (packages/nix/nix/patches/0014); upstream rnix should not take it before the Nix language change lands upstream.";
        };
      };
    }
  ];
}
