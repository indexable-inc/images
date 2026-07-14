# `nix run .#rebase-patches [-- <name>]`: regenerate a de-forked package's
# `patches/` series when its upstream base moves, by round-tripping through a
# real `git rebase`, plus the `dag.json` dependency graph next to each series
# and the `dag-check` invariant driver behind every `patch-dag-<name>` flake
# check. The mechanics live in the Rust crate (src/rebase.rs, src/dag.rs,
# src/check.rs); this file only wires runtime data in.
#
# The fork-package mapping (input name, upstream URL, patch dir) is data from
# lib/fork-packages.nix, rendered to JSON and baked in via the wrapper env, so
# the binary hardcodes no per-package coordinates. A downstream repo (e.g. ix)
# that keeps its own fork mapping + patches reuses this one tool by pointing it
# at its list: `nix run <index>#rebase-patches -- --mapping <its-fork.json>
# [<name>]`, run from its repo root so `patchDir` and `flake.lock` resolve
# there. One tool, parameterized by data, never copied per repo.
{
  ix,
  lib,
  formats,
  makeWrapper,
  runCommand,
  symlinkJoin,
  git,
  mergiraf,
}: let
  bin = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "rebase-patches";
    meta = {
      description = "Regenerate a de-forked package's patch series via a real git rebase when its upstream base moves, and its dependency DAG";
      license = lib.licenses.mit;
      mainProgram = "rebase-patches";
    };
  };
  # Fork-package mapping from the single source of truth (lib/fork-packages.nix,
  # surfaced as `ix.forkPackages`), rendered to JSON and baked in as a store
  # path.
  forkData = (formats.json {}).generate "fork-packages.json" ix.forkPackages;
  package = symlinkJoin {
    name = "rebase-patches";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];
    # git for the rebase round-trip; mergiraf as the syntax-aware merge driver.
    # No pinned nix: flake.lock is read as plain JSON, so no nix invocation.
    postBuild = ''
      # shell
      wrapProgram $out/bin/rebase-patches \
        --set REBASE_PATCHES_FORK_MAPPING ${forkData} \
        --prefix PATH : ${lib.makeBinPath [git mergiraf]}
    '';
    inherit (bin) meta;
    passthru =
      (builtins.removeAttrs bin.passthru ["unchecked"])
      // {
        tests =
          (bin.passthru.tests or {})
          // {
            resume = runCommand "rebase-patches-resume-test" {
              nativeBuildInputs = [git package];
            } (builtins.readFile ./resume-test.sh);
          };
      };
  };
in
  package
