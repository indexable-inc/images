# `nix run .#upstream-pr -- <pkg> <patch> [--open] [--dry-run]`: contribute one
# fork patch upstream (see packages/upstream-sync/src/bin/upstream_pr.rs for
# the full design doc: dag.json ancestor closure, scratch clone + git am
# --3way, fork branch push, draft PR with AI attribution).
#
# The binary lives in the upstream-sync crate (shared mapping/dag/patch
# modules; that package owns the crate's tests and policy checks). This
# wrapper owns only the Nix seams: the rendered fork-package mapping
# (UPSTREAM_SYNC_FORK_PACKAGES; downstream repos override with --mapping) and
# the runtime tools.
{
  ix,
  lib,
  formats,
  git,
  gh,
  coreutils,
  makeWrapper,
  symlinkJoin,
  ...
}: let
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  bin = ix.rustWorkspace.units.binaries.upstream-pr;
in
  symlinkJoin {
    name = "upstream-pr";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];
    postBuild = ''
      # shell
      wrapProgram $out/bin/upstream-pr \
        --prefix PATH : ${lib.makeBinPath [git gh coreutils]} \
        --set-default UPSTREAM_SYNC_FORK_PACKAGES ${forkData}
    '';
    meta = {
      description = "Contribute one fork patch upstream (its dag.json ancestor closure) via a fork branch + compare URL";
      mainProgram = "upstream-pr";
    };
  }
