{
  lib,
  btop,
  ix,
  nix,
  # Sibling package set (flake path only), for the `rebase-patches` binary the
  # fork updater invokes. `{ }` on the overlay path.
  repoPackages ? {},
  # Nushell writer for `passthru.updateScript`, pre-bound on the flake path
  # (lib/packages.nix); `null` on the overlay path -> omit the fork updater.
  updateScriptWriter ? null,
}:
# Upstream aristocratos/btop (btop-src input) with the in-repo patch series
# (./patches) applied: macOS process disk IO sorting, and kernel cwd in the
# process detail box. De-forking replacement for the old `indexable-inc/btop`
# pinned input.
btop.overrideAttrs (old: {
  src = ix.patchedSrc {
    name = "btop";
    src = ix.btopSrc;
    patchDir = ./patches;
  };

  # Fork updater (flake path only): bump btop-src and regenerate the patch
  # series, so btop joins the registry-discovered `.#update` DAG.
  passthru =
    (old.passthru or {})
    // lib.optionalAttrs (updateScriptWriter != null && repoPackages ? rebase-patches) {
      updateScript =
        ix.mkForkUpdater {
          writeNushellApplication = updateScriptWriter;
          inherit nix;
          rebasePatches = repoPackages.rebase-patches;
        } {
          name = "btop";
          input = "btop-src";
        };
    };

  meta =
    old.meta
    // {
      homepage = "https://github.com/aristocratos/btop";
    };
})
