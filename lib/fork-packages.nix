# Single source of truth for the de-forked packages: each one pins an upstream
# `flake = false` input and keeps its delta as an ordered `patches/` series
# next to the package (see lib/util/patched-src.nix). One list drives three
# consumers so they cannot drift:
#
#   - `packages/<...>/default.nix` applies the series via `ix.patchedSrc`.
#   - `lib/per-system.nix` exposes each patched source as
#     `checks.<system>.patched-src-<name>` (the seconds-fast conflict gate).
#   - `packages/rebase-patches` reads the rendered JSON (input name, upstream
#     git URL, repo-relative patch dir) to regenerate the series through a real
#     `git rebase` when the pinned base moves.
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
{
  forkPackages = [
    {
      name = "codex";
      input = "codex-src";
      url = "https://github.com/openai/codex.git";
      patchDir = "packages/agent/codex/patches";
      autoUpdate = true;
    }
    {
      name = "btop";
      input = "btop-src";
      url = "https://github.com/aristocratos/btop.git";
      patchDir = "packages/terminal/btop/patches";
      autoUpdate = true;
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
    }
  ];
}
