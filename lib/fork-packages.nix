# Single source of truth for the maintained forks. Each fork lives in a real
# GitHub fork repo (`forkRepo`) whose `bookmark` points at the "megamerge"
# commit: every patch is a commit whose parents are its true dependencies,
# and the megamerge's tree equals the full series applied linearly (its
# parents are the DAG heads). The flake input (`input`) pins that megamerge
# commit, so builds fetch one plain git commit and never apply patches or
# need jj installed. Fork repos are maintained with jj (colocated); every
# bookmark push carries a permanent `refs/pins/<date>-<sha12>` ref in the
# same operation that bumps flake.lock, so every rev the lock has ever
# pinned stays fetchable after a rebase rewrites history. Never push a
# conflicted jj commit: git-based readers (GitHub, the fetchers) cannot
# parse jj's conflict encoding.
#
# One list drives the consumers so they cannot drift:
#
#   - `packages/<...>/default.nix` consumes the pinned input directly as src
#     (plus `ix.patchedSrc` for `derivedPatches`, where declared).
#   - `.github/workflows/fork-sync.yml` jj-rebases each autoUpdate fork onto
#     the upstream tip, pushes bookmark + pin ref, and floats the input.
#   - `packages/upstream-sync` reads the per-patch upstreaming intent and
#     per-repo `upstreamPolicy` to drive the upstreaming loop; a patch ships
#     upstream by pushing its commit (whose git ancestry IS its dependency
#     closure) from the fork repo.
#
# Adding a fork is one entry here plus the fork repo itself: fork the
# upstream on GitHub, commit the patch DAG, push the bookmark and a pin ref,
# pin the megamerge as the flake input.
#
# Fields:
#   name        : package id.
#   input       : flake input pinning the megamerge (flake.lock `locked.rev`;
#                 branch-loose for autoUpdate forks).
#   upstreamUrl : upstream git URL rebases target.
#   upstreamRef : optional upstream branch the fork's base sits on.
#                 upstream-sync anchors the series base on it (merge-base
#                 against its tip) and drift compares against it; fork-sync
#                 rebases target its tip. Default: the upstream's default
#                 branch (its HEAD symref), which is right for every fork
#                 except one based off a maintenance branch.
#   forkRepo    : GitHub `owner/name` of the maintained fork repo.
#   bookmark    : fork-repo branch holding the megamerge (`ix-patched`; one
#                 bookmark per series where a repo carries several, e.g.
#                 rnix-parser's ix-patched-0.12 / ix-patched-0.14).
#   derivedPatches : optional list of patches DERIVED BY NIX at build time
#                instead of committed to the fork repo -- for mechanical or
#                generated content (a stanza stamped into every manifest, a
#                tracked lockfile) that re-conflicts on every rebase when
#                kept as a commit. Each entry:
#                  name      : short id (names the generated patch derivation).
#                  generator : repo-relative path to a .nix file evaluating to
#                              `{pkgs, src, ...}: drv` whose output is a single
#                              unified-diff file produced at BUILD time from
#                              the fetched megamerge tree. A generator must
#                              fail loudly behind a structural guard (e.g.
#                              "every `[package]` manifest got the stanza,
#                              count > 0"), never silently no-op, and never
#                              bake in magic totals that go stale.
#                  reason    : one line, the patch's reason of record. Derived
#                              patches are not fork-repo commits, so the
#                              commit-body reason rule cannot cover them; this
#                              field does.
#                  upstream  : always "never". A derived patch is repo-local
#                              mechanical output, invisible to the fork repo
#                              and upstream-sync by construction, so it can
#                              never be rebased or sent upstream.
#                `ix.patchedSrc` applies the generator outputs on top of the
#                fetched megamerge tree; see lib/util/patched-src.nix.
#   autoUpdate : whether the scheduled fork-sync (.github/workflows/fork-sync.yml)
#                may jj-rebase the fork onto the upstream tip and float the
#                input under the cron. `false` pins the input by rev; it moves
#                only under a deliberate manual rebase.
#
# Upstreaming intent (hand-written declarative intent; the human gate on the
# outward act). `packages/upstream-sync` reads these; the LIVE state it tracks
# (PR urls, states, retirement) is generated, never hand-written.
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
#   patches        : per-patch intent, keyed by the patch commit's SUBJECT
#                    line (the identity that survives rebases; jj evolves the
#                    commit but the subject is the patch's name of record).
#                    Each value:
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
#                    automatically). `upstream-sync` fails loudly when an intent key
#                    matches no commit subject on the fork bookmark, and treats a repo
#                    whose `upstreamPolicy.aiPrsAllowed == false` as `never` regardless
#                    of the per-patch mark, so a banned repo cannot leak a PR.
#
# There is deliberately NO per-patch description field: the upstream PR's title
# and body come from the patch commit's own message (subject = title, body = PR
# body, plus AI attribution and a link to the patch of record; see
# packages/upstream-pr). A nix copy would duplicate the commit message and
# drift. One fact, one home: the commit message IS the patch's description and
# its reason of record; upstream-sync refuses to ship a commit whose body
# states no reason (attribution trailers and bare issue refs do not count).
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
      upstreamUrl = "https://github.com/openai/codex.git";
      forkRepo = "indexable-inc/codex";
      bookmark = "ix-patched";
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
        "mcp: route channel notifications into chat" = {
          upstream = "never";
          reason = "Hard to land upstream (fast-moving OpenAI-owned repo); ix-specific MCP channel-notification routing.";
        };
        "tui: refresh adaptive syntax theme on focus regain" = {
          upstream = "never";
          reason = "General fix for openai/codex#18942, but codex closes unsolicited PRs (prsWelcome = false); the upstream issue is the feedback channel.";
        };
      };
    }
    {
      name = "btop";
      input = "btop-src";
      upstreamUrl = "https://github.com/aristocratos/btop.git";
      forkRepo = "indexable-inc/btop";
      bookmark = "ix-patched";
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
        "Add macOS process disk IO sorting" = {
          upstream = "hold";
          reason = "General macOS feature (per-process disk IO sorting) plausibly welcome upstream, but wants a quality pass and a discussion issue first per btop CONTRIBUTING.";
        };
        "proc: show kernel working directory (cwd) in the detail box" = {
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
      # jj-rebase indexable-inc/home-manager, push bookmark + pin ref, repin.
      name = "home-manager";
      input = "home-manager-src";
      upstreamUrl = "https://github.com/nix-community/home-manager.git";
      forkRepo = "indexable-inc/home-manager";
      bookmark = "ix-patched";
      autoUpdate = false;
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
        "files: batch symlink creation and target checks in activation" = {
          upstream = "attempt";
          reason = "General activation performance fix (per-file fork+exec dominates linkGeneration/checkLinkTargets on darwin, 3.4s -> 0.2s for 365 links); behavior-preserving and upstream-quality.";
        };
      };
    }
    {
      name = "git";
      input = "git-src";
      upstreamUrl = "https://github.com/git/git.git";
      forkRepo = "indexable-inc/git";
      bookmark = "ix-patched";
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
        "submodule--helper: borrow common-dir module store in linked worktrees" = {
          upstream = "hold";
          reason = "General fix git upstream anticipated in df56607dff2 but never implemented; wants a mailing-list submission with review, which upstream-sync cannot automate for a non-PR project.";
        };
      };
    }
    {
      name = "nushell";
      input = "nushell-src";
      upstreamUrl = "https://github.com/nushell/nushell.git";
      forkRepo = "indexable-inc/nushell";
      bookmark = "ix-patched";
      autoUpdate = true;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nushell/nushell/blob/main/CONTRIBUTING.md";
        notes = "PRs welcome for focused changes; CONTRIBUTING has no AI-specific policy as of 2026-07-07. Include tests and user-facing release-note context.";
      };
      patches = {
        "Add xattrs column to ls -l" = {
          upstream = "attempt";
          reason = "General filesystem feature requested in nushell/nushell#7106; prior PR #7158 was abandoned and explicitly left open for takeover.";
          prExtra = "Related issue: nushell/nushell#7106. Prior closed attempt: nushell/nushell#7158.";
        };
        "Derive feature list for cargo-unit builds" = {
          upstream = "never";
          reason = "Repo-specific: cargo-unit does not export Cargo's aggregate CARGO_CFG_FEATURE env var, so the package derives it from CARGO_FEATURE_* for ix builds.";
        };
      };
    }
    {
      # Full ghostty application source (index#3768), the fork's single patch
      # point: `packages/ghostty`, `packages/tui/vt/libghostty-vt`, and the
      # Rust workspace's ix-vt link all build the VT-only subtree from this
      # series. 0003 adds C API that ix-vt's checked-in bindings reference,
      # so the patched source is load-bearing, not just validated.
      name = "ghostty";
      input = "ghostty-src";
      upstreamUrl = "https://github.com/ghostty-org/ghostty.git";
      forkRepo = "indexable-inc/ghostty";
      bookmark = "ix-patched";
      # Pinned by rev (see flake.nix's ghostty-src comment): a routine bump
      # can silently break the darwin-only build with no CI to catch it.
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        # AI_POLICY.md: AI-assisted contributions are welcome with full
        # disclosure and human review. CONTRIBUTING.md additionally runs a
        # vouch system: any first-time contributor's PR is auto-closed until
        # a maintainer comments `!vouch` on a "Vouch Request" discussion, so
        # `upstream-sync` cannot open a PR here until that human step happens.
        aiPrsAllowed = "true";
        citation = "https://github.com/ghostty-org/ghostty/blob/main/AI_POLICY.md";
        notes = "AI-assisted PRs welcome with disclosure (AI_POLICY.md) but gated by CONTRIBUTING.md's vouch system: unvouched contributors are auto-closed, so a human must request and receive a maintainer's `!vouch` before upstream-sync can open a PR.";
      };
      patches = {
        "macos: fire undo-close expiry via main-queue GCD timer" = {
          upstream = "attempt";
          reason = "Undo-close retention (ghostty-org/ghostty#7535) leaks live sessions when the run-loop expiry Timer never fires; a main-queue GCD timer fires regardless of arming thread and run-loop mode.";
          prExtra = "Context: undo-close design in ghostty-org/ghostty#7535; observed dozens of hidden surfaces tens of hours old against a 5s undo-timeout.";
        };
        "termio: hang up child process groups when spawn-time killpg EPERMs" = {
          upstream = "attempt";
          reason = "Darwin killpg EPERM at surface close can mean the hangup reached nobody (root-owned login(1) alone in the spawn-time group; the shell moved to its own job-control group), leaving shells alive against a revoked tty.";
          prExtra = "Related earlier lifecycle issues: ghostty-org/ghostty#2273, ghostty-org/ghostty#4554.";
        };
        "terminal: expose per-cell OSC 8 hyperlink URIs through the render state" = {
          upstream = "attempt";
          reason = "The render-state C API exposes only a per-cell has-hyperlink bool (the row flag may false-positive), so a libghostty-vt embedder cannot learn a cell's OSC 8 link target; the URI must be duplicated out of page memory during update() like graphemes and styles already are.";
          prExtra = "Motivating embedder: ix-term (indexable-inc/ix#8008), which renders the terminal grid in a browser and needs real anchors for OSC 8 hyperlinks.";
        };
        "build: keep -Demit-lib-vt buildable without Apple absolute-path tools" = {
          upstream = "attempt";
          reason = "A -Demit-lib-vt build cannot exec hardcoded /bin/cp and /usr/bin/ranlib in a hermetic (Nix darwin sandbox) build, and hanging the vt xcframework off install forces xcodebuild on every darwin build; resolving the tools from PATH and gating the xcframework on emit-xcframework (already default-false under emit-lib-vt) matches how the app xcframework is gated and changes nothing for developer Macs.";
          prExtra = "Motivating embedder: ix-vt (indexable-inc/ix#8117), which builds the VT-only subtree fully sandboxed; lib/build/libghostty-vt.nix documents the boundary.";
        };
      };
    }
    {
      # clippy is nightly-toolchain-coupled: its input is pinned by rev and must
      # move only with the pinned nightly, so the fork is jj-rebased explicitly
      # alongside a toolchain bump, never under a blanket `nix flake update` or
      # the scheduled fork-sync. `name` is `clippy` (not the `llm-clippy` package
      # id) so the check reads `patched-src-clippy` and the rebase arg is `clippy`.
      name = "clippy";
      input = "clippy-src";
      upstreamUrl = "https://github.com/rust-lang/rust-clippy.git";
      forkRepo = "indexable-inc/rust-clippy";
      bookmark = "ix-patched";
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
        "Update Clippy for nightly 2026-05-27 (repo toolchain pin)" = {
          upstream = "never";
          reason = "Repo-specific: pins clippy to our nightly toolchain; not an upstream change.";
        };
        # The ten new lints: the user's default is `attempt` but HOLD for a
        # quality pass (a lint upstream needs a proposal issue, docs, ui tests,
        # and a stabilization discussion). The clippy quality pass is a follow-up.
        "Add module_file_count lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate but wants a quality pass (lint proposal issue, docs, ui tests) before a PR.";
        };
        "Add excessive_file_length lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add path_segment_repetition lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add underscore_in_module_filename lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add renamed_imports lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add fallible_int_fallback lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add magic_number lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add drop_must_use lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add non_trait_imports lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add string_ip_field lint" = {
          upstream = "hold";
          reason = "New lint; attempt candidate pending the quality pass.";
        };
        "Add anonymous tuple return type lint" = {
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
      # gitlab remote stays the rebase target for indexable-inc/mesa; the
      # build consumes `ix.mesaSrc` (the shallow git input) through patchedSrc.
      name = "mesa";
      input = "mesa-src";
      upstreamUrl = "https://gitlab.freedesktop.org/mesa/mesa.git";
      forkRepo = "indexable-inc/mesa";
      bookmark = "ix-patched";
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
        "venus: handle temporary sync fd semaphore imports driver-side" = {
          upstream = "never";
          reason = "Real venus driver fix and a strong upstream candidate, but mesa is GitLab: upstream-sync's gh path cannot open the MR. Contribute by hand.";
        };
        "README.ix: document snapshot fork layout" = {
          upstream = "never";
          reason = "Repo-specific: documents our snapshot fork layout; meaningless upstream.";
        };
        "venus: fail sparse batches waiting on driver-side sync fd imports" = {
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
      # `nix-src` rev after jj-rebasing indexable-inc/nix.
      name = "nix";
      input = "nix-src";
      upstreamUrl = "https://github.com/NixOS/nix.git";
      # The 2.34.7 base lives on the maintenance branch, not master.
      upstreamRef = "2.34-maintenance";
      forkRepo = "indexable-inc/nix";
      bookmark = "ix-patched";
      autoUpdate = false;
      # nix is the fork with the most attempt patches; each ships upstream as
      # its commit's ancestry from indexable-inc/nix. Preflight before an
      # upstream PR: build the fork at that patch commit's rev.
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
        # jj input scheme (nix#15651): vendored from the already-open upstream
        # PR #16066 (ee0691ab) with three 2.34.7 back-port adaptations noted in
        # the commit body. No PR of ours: upstream's own is in flight; adopt
        # theirs (and drop the adaptations) when the base moves past it.
        "libfetchers: add a Jujutsu (jj) input scheme" = {
          upstream = "hold";
          reason = "Upstream PR NixOS/nix#16066 (same commit, ee0691ab) is already open; never file a rival. Re-sync when the base catches up.";
        };
        # Real product fix, upstream-shaped and standalone: on Darwin/FreeBSD
        # LOCAL_PEERCRED carries no pid, so the daemon never recorded
        # clientPid (build attribution + disconnect liveness silently degraded).
        "libcmd: report peer pid on Darwin/FreeBSD via LOCAL_PEERPID" = {
          upstream = "hold";
          reason = "Genuine upstream bug fix (daemon clientPid always missing on Darwin); wants a human review pass before an attempt mark authorizes the PR.";
        };
        # Fork-local test adaptations: they exist because fork patches changed
        # failure propagation / added features, so they are meaningless upstream.
        "tests/functional: update failure expectations for preserved leaf errors" = {
          upstream = "never";
          reason = "Adapts upstream tests to the fork's failure-propagation patches; upstream has nothing to apply it to.";
        };
        "tests/functional: assert no-progress deadline only where it exists" = {
          upstream = "never";
          reason = "Tests a fork-only feature (no-progress deadlines).";
        };
        "tests/functional: skip zombie staleness check where states are hidden" = {
          upstream = "never";
          reason = "Fixes a fork-added test (build-status liveness) for the macOS seatbelt.";
        };
        "fix(libstore): don't crash the daemon when a GC roots client thread is interrupted" = {
          upstream = "hold";
          reason = "Reworked to catch (BaseError&) per the #15963 review (xokdvium: `catch (...)` swallows too much); a human (Andrew) resubmits, referencing #15963/#15962/#13438. Fixes NixOS/nix#15962.";
        };
        # 0002: the cleanest single-file candidate -- a regression restoration.
        # #8240 made nix's default-path probing EPERM/EACCES-tolerant on the
        # macOS sandbox (treat permission-denied like absent); the later
        # std::filesystem migration reintroduced the throwing exists() overload
        # that #5884 first flagged and #8485 still tracks. A human submits it
        # framed as restoring that lost behavior.
        "fix(libexpr): treat inaccessible default lookup-path entries as absent" = {
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
        "libutil: add build-status-dir experimental feature" = {
          upstream = "hold";
          reason = "Build-status series overlaps edolstra's active #15979 (nix ps); engage there with the daemon-less file-based angle instead of filing a competing series.";
        };
        "libstore: add build status directory writer" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "libstore: write status files from build and substitution goals" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "libstore: daemon record client identity for build status" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "nix: add 'nix store builds' command" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "tests/functional: test build status directory" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        "doc: release note for build status directory and 'nix store builds'" = {
          upstream = "hold";
          reason = "Build-status series: engage on #15979 rather than open a competing PR.";
        };
        # Structured git history export (RFC 0011). Designed to be
        # upstreamable (deterministic, opt-in, experimental-feature gated,
        # never in lock files -- it dodges the objections that sank
        # leaveDotGit-for-flakes), but held: repo-wide upstreaming pause
        # (NixOS/nix#15984, see #2021), and a feature of this size should
        # start as an upstream discussion, not a cold PR.
        "libfetchers: add opt-in structured commit history export for git inputs" = {
          upstream = "hold";
          reason = "Feature-sized change; upstreaming paused per NixOS/nix#15984 and it should open as an upstream issue/RFC first.";
        };
        # 0011: temp roots for in-flight CA build outputs, closing the min-free
        # auto-GC race that broke wide cargo-unit graphs (index#2334).
        "fix(libstore): add temp roots for CA derivation outputs (GC race)" = {
          upstream = "hold";
          reason = "Fix for min-free auto-GC deleting in-flight CA build outputs (indexable-inc/index#2334). Hold: humans submit nix patches upstream per NixOS/nix#15984; overlaps the still-open upstream discussion NixOS/nix#15613 / NixOS/nix#15719.";
        };
        # 0012: temp root for the floating-CA scratch output path itself
        # (makeFallbackPath), the residual GC window 0011 left open: a
        # non-chroot builder writes the unregistered scratch path directly,
        # and a concurrent GC deletes it mid-build (index#2354).
        "fix(libstore): add temp root for floating-CA scratch output paths (GC race)" = {
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
        "libfetchers: opt-in incremental fetching of forge inputs via the Git protocol" = {
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
        # enforced by the `checks.<system>.stock-nix-parse-*` shards (index#3635).
        # astlog's digit-grouping lints track the remaining toolchain
        # backlog (astlog-rules/nix.astlog).
        "libexpr: accept underscore digit separators in numeric literals" = {
          upstream = "hold";
          reason = "Language syntax change; must start as an upstream issue/RFC, and humans submit nix patches upstream per NixOS/nix#15984.";
        };
        "fix(libcmd): preserve repeated installable cardinality" = {
          upstream = "hold";
          reason = "Fixes repeated installables multiplying `nix build --json` results (indexable-inc/index#2633). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0016: a newer Nix uses opaque per-instance temporary-root filenames.
        # The 2.34 collector parsed every entry as a decimal PID, so one newer
        # file disabled both scheduled and reactive GC until the store filled
        # (index#3031). Upstream master already treats the name as opaque in
        # NixOS/nix#15992; this is the reader-side backport for mixed versions.
        "fix(libstore): accept opaque temporary root filenames" = {
          upstream = "hold";
          reason = "Backports the mixed-version temporary-root reader from NixOS/nix#15992 (indexable-inc/index#3031). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0017: each daemon process decides whether to auto-GC before waiting
        # for the store-global gc.lock. Recheck under that lock so queued
        # callers do not repeat a collection after the first restores space.
        "fix(libstore): recheck free space after GC lock" = {
          upstream = "hold";
          reason = "Prevents stale queued auto-GC decisions from serializing CI jobs behind repeated collections (indexable-inc/index#3085, indexable-inc/ix#7145). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0018: a daemon worker loses its signal thread at fork while retaining
        # the blocked mask, then synchronous auto-GC can wait forever on a
        # detached collector queued at gc.lock after the client disappears.
        "fix(libstore): interrupt blocked automatic GC" = {
          upstream = "hold";
          reason = "Restarts forked daemon signal handling and makes blocked auto-GC observe cancellation (indexable-inc/index#3300, indexable-inc/ix#7145). Signal handling ports Lix dccde9436; humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        "Get rid of duplicated 'Build failed due to failed dependency' error messages" = {
          upstream = "hold";
          reason = "Backports NixOS/nix#16040 from 2.34.8; drop when the daemon base reaches 2.34.8 or newer.";
        };
        "libstore: preserve content-addressed leaf failures" = {
          upstream = "hold";
          reason = "Preserves actionable floating-CA leaf failures through resolution (indexable-inc/index#3279, indexable-inc/ix#7357). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0021: CPU, I/O, and build-log activity distinguish a silent active
        # compiler from an idle builder before the daemon cancels its goal.
        "libstore: enforce derivation no-progress deadlines" = {
          upstream = "hold";
          reason = "Fleet CI policy for indexable-inc/index#3317. Validate the process-aware deadline before proposing a general Nix interface; humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0022: a paused (backpressured) substitution download neither reads
        # its socket nor advances CURLOPT_LOW_SPEED_TIME, so a peer half-close
        # parked the transfer forever -- nix-daemon children stranded in
        # CLOSE-WAIT wedged CI slots for hours. The worker loop now polls
        # paused transfers and fails them as transient curl errors, so
        # download-attempts still applies.
        "libstore: fail paused downloads on peer half-close or stall" = {
          upstream = "hold";
          reason = "Fails paused downloads on peer half-close or stall past stalled-download-timeout instead of parking forever (indexable-inc/index#3559). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0023: forge archive inputs (github:/gitlab:/sourcehut:) requesting
        # submodules or LFS are constructed as the equivalent git+https input
        # (archive tarballs cannot contain that data), so lock files record a
        # plain `git` node stock Nix understands. Fixes the hard failure of
        # NixOS/nix#13571 and the silent empty-submodule trees of
        # NixOS/nix#14982; the mapping mirrors GitHubInputScheme::clone().
        "libfetchers: fetch forge inputs via git when submodules are requested" = {
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
        "libflake: defer to the child lock for relative path flake inputs" = {
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
        "libexpr: Add a way to collect string context from ValuePrinter" = {
          upstream = "hold";
          reason = "Backports upstream 569ee752c (prerequisite of NixOS/nix#15711, merged to master 2026-04-27); drop when the base reaches a release containing it (2.35+).";
        };
        "Don't copy flakes to the store unnecessarily" = {
          upstream = "hold";
          reason = "Backports upstream 891ef140b (NixOS/nix#15711, merged to master 2026-04-27); drop when the base reaches a release containing it (2.35+).";
        };
        "libexpr: Make hash mismatches while copying lazy paths to the store a proper error" = {
          upstream = "hold";
          reason = "Backports upstream 8ffda0826 (NixOS/nix#15950, post-merge fix to #15711); drop when the base reaches a release containing it (2.35+).";
        };
        "libexpr: Handle lazy paths in builtins.storePath better" = {
          upstream = "hold";
          reason = "Backports upstream 933f3140b (NixOS/nix#16078, post-merge fix to #15711); drop when the base reaches a release containing it (2.35+).";
        };
        # 0029: our divergence from upstream master, which enables lazy
        # mounting unconditionally. The off-by-default `lazy-trees` setting
        # keeps the fork byte-identical to eager evaluation unless opted in;
        # flipping the fleet on is a separate decision with its own drv-hash
        # and eval-result equivalence sweeps (indexable-inc/index#3645).
        "libexpr: gate lazy input mounting behind an off-by-default lazy-trees setting" = {
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
        "libflake: stamp submodule metadata on relative path flake inputs" = {
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
        "fix: libfetchers: keep user git auto-maintenance out of cache fetches" = {
          upstream = "hold";
          reason = "Keeps user git auto-maintenance out of nix-internal cache repos; fetch-spawned detached `git maintenance run --auto` SIGSEGVs on macOS (indexable-inc/index#3755). Upstream-nix candidate, held: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # With lazy-trees on, path inputs and dirty git worktrees are read
        # from the live filesystem for the whole eval, so a concurrent
        # writer kills long evals with "contents have changed" (exit 102).
        # Snapshot such trees at mount time via clonefile(2) (~70ms) and
        # evaluate from the snapshot; fall back to an eager copy where
        # cloning is unavailable.
        "libexpr: snapshot mutable source trees at mount time (lazy-trees)" = {
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
        "libstore: wake the goal loop when the process is interrupted" = {
          upstream = "hold";
          reason = "Level-triggered interrupt wakeup for the goal loop (indexable-inc/index#3752); upstream master's Waker pipe is not interrupt-wired, so the bug exists there too. Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0034: `nix store builds` pruned dead writers with kill(pid, 0),
        # which still succeeds for zombies -- 33 phantom "in flight" builds
        # owned by three zombie workers survived 10.5h on hydra
        # (indexable-inc/index#3752). Writers now hold a lifetime flock on
        # their entry; readers treat an acquirable lock (or, for legacy
        # entries, a dead-or-zombie pid) as proof the writer is gone.
        "libstore: prove build-status writers alive with a lifetime lock" = {
          upstream = "hold";
          reason = "Build-status series follow-up (zombie-proof staleness, indexable-inc/index#3752): engage on #15979 rather than open a competing PR.";
        };
        # 0035: MonitorFdHup's poll branch watched only POLLHUP, but POLLERR/
        # POLLNVAL arrive regardless of the events mask, so an error-state
        # client socket spun the monitor thread at 100% CPU without ever
        # signalling client death (linux poll path only; darwin uses kqueue).
        "libutil: treat POLLERR/POLLNVAL as fd death in MonitorFdHup" = {
          upstream = "hold";
          reason = "Port of the MonitorFdHup half of NixOS/nix#15691 (open since 2026-04, indexable-inc/index#3769); retire if that PR merges. Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0036: the progress bar emits the ConEmu-style progress OSC (9;4) so
        # terminals that understand it (Ghostty, WezTerm, Windows Terminal,
        # ConEmu) show a native build-progress indicator; percent mirrors the
        # build + copy counters of the textual status line.
        "libmain: emit terminal progress (OSC 9;4) from the progress bar" = {
          upstream = "hold";
          reason = "UX feature (indexable-inc/index#3830) upstream may want behind a setting or with broader terminal detection. Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # 0037: the status line never repaints on SIGWINCH (redraw() dedupes
        # and nothing invalidates on resize), so resizing the terminal leaves
        # a stale truncated or wrapped line; adds a window-size callback in
        # libutil and registers the progress bar to repaint through it.
        "libmain: repaint the progress bar on terminal resize" = {
          upstream = "hold";
          reason = "Real upstream bug worth a PR (stale progress line after resize, indexable-inc/index#3830). Hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
        # builtins.wasm, imported from the open upstream PR NixOS/nix#15380
        # plus the fleet's determinism hardening on top
        # (indexable-inc/index#3997).
        "Add builtins.wasm" = {
          upstream = "hold";
          reason = "Import of the open upstream PR NixOS/nix#15380 (edolstra's builtins.wasm); nothing to send back, engage on that PR. Retire when the base ships it.";
        };
        "libexpr: force deterministic Wasm execution in builtins.wasm" = {
          upstream = "hold";
          reason = "Fleet determinism policy on top of the #15380 import (NaN canonicalization + deterministic relaxed SIMD keep eval bit-identical across darwin/linux); upstream measured ~3.6x on float-heavy Wasm and left it off, so propose it on the PR rather than fork-PR it.";
        };
        # Rev-pinned fetchGit inputs stop paying a network `git ls-remote
        # HEAD` per eval once cached (indexable-inc/index#4028).
        "libfetchers: resolve git refs lazily and refresh the cached HEAD" = {
          upstream = "hold";
          reason = "Fixes the eval-time network round trips of NixOS/nix#13556 (getDefaultRef never cached effectively) for rev-pinned fetchGit inputs (indexable-inc/index#4028). Upstream-nix candidate; hold: humans submit Nix patches upstream per NixOS/nix#15984.";
        };
      };
    }
    {
      # nix-fast-build is the CI build engine (the `check` app): the package
      # overlays this patched source onto nixpkgs' nix-fast-build recipe
      # (packages/nix/nix-fast-build), so the base must equal the nixpkgs
      # package version (tag 1.6.0), never free-float under the fork-sync
      # cron. On a nixpkgs nix-fast-build bump, repin the input to the
      # matching tag after jj-rebasing indexable-inc/nix-fast-build.
      name = "nix-fast-build";
      input = "nix-fast-build-src";
      upstreamUrl = "https://github.com/Mic92/nix-fast-build.git";
      forkRepo = "indexable-inc/nix-fast-build";
      bookmark = "ix-patched";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/Mic92/nix-fast-build";
        notes = "No CONTRIBUTING or AI policy published as of 2026-07-19; small focused PRs with tests are the observed norm.";
      };
      patches = {
        "workers: make --skip-cached skip locally-realized outputs" = {
          upstream = "hold";
          reason = "Changes what --skip-cached means for `local` outputs for every user; upstream would plausibly want it opt-in, so it needs reshaping as a flag before a PR.";
        };
        "build: add a typed per-derivation no-progress deadline" = {
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
      # jj-rebasing indexable-inc/Haskell-Nix-Derivation-Library.
      name = "nix-derivation";
      input = "nix-derivation-src";
      upstreamUrl = "https://github.com/Gabriella439/Haskell-Nix-Derivation-Library.git";
      forkRepo = "indexable-inc/Haskell-Nix-Derivation-Library";
      bookmark = "ix-patched";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/Gabriella439/Haskell-Nix-Derivation-Library";
        notes = "No CONTRIBUTING or AI policy; the CA gap is tracked upstream as issue #28 with PR #26 proposing a sum-type DerivationOutput.";
      };
      patches = {
        "Parser: accept empty output paths in floating-CA derivations" = {
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
      # entries pin the megamerges built on the upstream tags the in-use
      # crates were cut from -- v0.12.0 (alejandra, deadnix) here, v0.14.0
      # (statix) below; the build-time patcher overlays the patched sources
      # from these inputs. Repin alongside the vendored-version change on a
      # nixpkgs bump (the build fails loudly on an unknown rnix version).
      name = "rnix-0-12";
      input = "rnix-0-12-src";
      upstreamUrl = "https://github.com/nix-community/rnix-parser.git";
      forkRepo = "indexable-inc/rnix-parser";
      bookmark = "ix-patched-0.12";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nix-community/rnix-parser";
        notes = "nix-community project, PRs welcome, no stated AI policy; the patched dialect is gated on the Nix language itself changing, and 0.12 is a historical tag that upstream would not amend anyway.";
      };
      patches = {
        "tokenizer: accept underscore digit separators in numeric literals" = {
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
      upstreamUrl = "https://github.com/nix-community/rnix-parser.git";
      forkRepo = "indexable-inc/rnix-parser";
      bookmark = "ix-patched-0.14";
      autoUpdate = false;
      upstreamPolicy = {
        prsWelcome = true;
        aiPrsAllowed = "unknown";
        citation = "https://github.com/nix-community/rnix-parser";
        notes = "nix-community project, PRs welcome, no stated AI policy; the patched dialect is gated on the Nix language itself changing.";
      };
      patches = {
        "tokenizer: accept underscore digit separators in numeric literals" = {
          upstream = "hold";
          reason = "Lexes a dialect only index's patched nix accepts (packages/nix/nix/patches/0014); upstream rnix should not take it before the Nix language change lands upstream.";
        };
      };
    }
  ];
}
