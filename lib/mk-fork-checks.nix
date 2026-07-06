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
#   dagCheckSrc  : a directory holding `dag-check.nu` + `dag-lib.nu` (index's
#                  `packages/rebase-patches`), the shared DAG driver + verifier.
{
  lib,
  pkgs,
  patchedSrcFor,
  forkPackages,
  forkSrcInputs,
  patchesRoot,
  flakeLock,
  dagCheckSrc,
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
  # regenerating reproduces the committed bytes). Pure text work on the fetched
  # src tree in the sandbox, so it stays seconds-fast. The derivation and
  # verification logic is owned by `dagCheckSrc` (dag-{lib,check}.nu); the check
  # just wires the src, patch dir, and pinned rev into that driver.
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
          nativeBuildInputs = [
            pkgs.nushell
            pkgs.git
            # `chmod` (external) to make the read-only store src writable before
            # the apply-tests, since git must write files during `am`.
            pkgs.coreutils
          ];
        }
        ''
          # nushell's `use` resolves modules relative to the script file, so run
          # the driver from a dir holding both it and dag-lib.nu.
          workdir=$(mktemp -d)
          cp ${dagCheckSrc}/dag-check.nu ${dagCheckSrc}/dag-lib.nu "$workdir/"
          # git needs an identity even for the throwaway base commit.
          export HOME="$workdir"
          nu "$workdir/dag-check.nu" \
            ${lib.escapeShellArg (toString forkSrcInputs.${fork.name})} \
            ${patchDirStore} \
            ${lib.escapeShellArg expectedBase}
          touch "$out"
        ''
      )
  );
in
  patchedSrcChecks // patchDagChecks
