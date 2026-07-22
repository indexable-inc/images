# `nix run .#upstream-sync`: drive the de-fork UPSTREAMING loop (see
# packages/upstream-sync/src/main.rs for the full design doc: declarative
# intent in lib/fork-packages.nix, the patch series read live from each fork
# repo's commit DAG, generated live state in packages/upstream-sync/status/,
# double gating of the outward PR-opening act, and the `drift` companion
# report the fork-sync cron consumes).
#
# This wrapper owns only the Nix seams the binary cannot: the rendered
# fork-package mapping (UPSTREAM_SYNC_FORK_PACKAGES; downstream repos override
# with --mapping) and the runtime tools, including the sibling `upstream-pr`
# mechanism it delegates to. git talks to the fork repos and upstreams; gh
# reads/opens PRs.
{
  ix,
  lib,
  formats,
  git,
  gh,
  coreutils,
  # Sibling repo packages, threaded under one name (see lib/packages.nix); we take
  # the PR mechanism (`upstream-pr`) from here rather than a bare callPackage arg,
  # which the package set does not expose flat.
  repoPackages,
  makeWrapper,
  symlinkJoin,
  ...
}: let
  inherit (repoPackages) upstream-pr;
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  bin = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "upstream-sync";
    meta = {
      description = "Drive the de-fork upstreaming loop: track PR state, find duplicates, retire merged patches, and open PRs for attempt-marked patches";
      mainProgram = "upstream-sync";
    };
  };
in
  symlinkJoin {
    name = "upstream-sync";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];
    postBuild = ''
      # shell
      wrapProgram $out/bin/upstream-sync \
        --prefix PATH : ${lib.makeBinPath [git gh coreutils upstream-pr]} \
        --set-default UPSTREAM_SYNC_FORK_PACKAGES ${forkData}
    '';
    inherit (bin) meta;
    passthru = builtins.removeAttrs bin.passthru ["unchecked"];
  }
