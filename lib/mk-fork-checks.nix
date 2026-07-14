# Build the de-forked-package flake checks (`patched-src-<name>` and
# `patch-dag-<name>`) for a repo's fork list. Factored out of lib/per-system.nix
# so the SAME builder serves both this repo (index) and a downstream consumer
# (e.g. ix) that keeps its own fork mapping + patches but reuses index's
# machinery via `inputs.index.lib.mkForkChecks`. One owner for the check
# derivations means the two repos can never drift on how a series is validated.
#
# All repo-specific data is an argument; nothing about index's own forks is baked
# in here:
#   pkgs         : the target-system package set (for applyPatches / runCommand).
#   patchedSrcFor: the `lib.patchedSrcFor pkgs` binding (see lib/util/patched-src.nix).
#   forkPackages : the repo's fork mapping list (name / input / url / patchDir).
#   forkSrcInputs: `name -> raw upstream src` (the `flake = false` inputs), keyed
#                  by `fork.name`, so the check consumes the exact tree the build
#                  patches.
#   patchesRoot  : repo root the `fork.patchDir` (repo-relative) resolves against.
#   flakeLock    : the repo's parsed `flake.lock` (for the pinned base rev per
#                  input, validated against the committed dag.json base).
#   rebasePatches: the built `packages/rebase-patches` tool (index's
#                  `packages.<system>.rebase-patches`), whose `dag-check`
#                  subcommand is the shared DAG driver + verifier.
{
  lib,
  pkgs,
  patchedSrcFor,
  forkPackages,
  forkSrcInputs,
  patchesRoot,
  flakeLock,
  rebasePatches,
}: let
  # `patched-src-<name>`: the seconds-fast "does the series still apply" gate.
  # Built from the same `patchedSrcFor` the packages consume against the same raw
  # upstream inputs, so the check can never drift from the real build: green here
  # means the build gets an identical patched tree.
  patchedSrcChecks = lib.genAttrs' forkPackages (
    fork:
      lib.nameValuePair "patched-src-${fork.name}" (
        patchedSrcFor {
          inherit (fork) name;
          src = forkSrcInputs.${fork.name};
          patchDir = patchesRoot + "/${fork.patchDir}";
        }
      )
  );

  # `patch-dag-<name>`: the fast textual sibling of `patched-src-<name>`. Where
  # `patched-src` proves the linear series still applies, this proves the
  # committed `dag.json` is honest and in sync (declared ancestors sufficient,
  # independent patches commute byte-for-byte, NNNN is a topological order, and
  # regenerating reproduces the committed bytes), and that the fork's upstreaming
  # intent (`fork.patches`, if declared) is coherent: keys name real patch files.
  # It also fails any patch that states no reason in its commit-message body:
  # the body is the reason of record for every fork patch and, for
  # attempt-marked ones, the upstream PR description (see packages/upstream-pr). Pure text work on the
  # fetched src tree in the sandbox, so it stays seconds-fast. The derivation and
  # verification logic is owned by `rebasePatches` (packages/rebase-patches
  # src/{dag,check}.rs); the check just wires the src, patch dir, pinned rev,
  # and intent into that driver.
  patchDagChecks = lib.genAttrs' forkPackages (
    fork: let
      expectedBase = flakeLock.nodes.${fork.input}.locked.rev;
      # Import the committed patch series + dag.json into the store so the sandbox
      # can read them (the raw repo path is not a sandbox input).
      patchDirStore = builtins.path {
        name = "${fork.name}-patches";
        path = patchesRoot + "/${fork.patchDir}";
      };
    in
      lib.nameValuePair "patch-dag-${fork.name}" (
        pkgs.runCommand "patch-dag-${fork.name}-check"
        {
          # git for the apply-tests (the wrapper's PATH prefix serves `nix run`,
          # not this sandbox); the driver seeds its own throwaway identity.
          nativeBuildInputs = [
            rebasePatches
            pkgs.git
          ];
        }
        ''
          # shell
          rebase-patches dag-check \
            ${lib.escapeShellArg (toString forkSrcInputs.${fork.name})} \
            ${patchDirStore} \
            ${lib.escapeShellArg expectedBase} \
            ${lib.escapeShellArg (builtins.toJSON (fork.patches or {}))}
          touch "$out"
        ''
      )
  );
in
  patchedSrcChecks // patchDagChecks
