{
  ix,
  lib,
  nix,
  # Sibling package set (flake path only), for the `rebase-patches` binary the
  # fork updater invokes. `{ }` on the overlay path.
  repoPackages ? {},
  # Nushell writer for `passthru.updateScript`, pre-bound on the flake path
  # (lib/packages.nix); `null` on the overlay path -> omit the fork updater.
  updateScriptWriter ? null,
}: let
  source = ix.patchedSrc {
    name = "nushell";
    src = ix.nushellSrc;
    patchDir = ./patches;
  };

  workspace = ix.cargoUnit.buildWorkspace {
    pname = "nushell";
    src = source;
    workspaceRoot = source;
    cargoLock = source + "/Cargo.lock";
    cargoArgs = [
      "-p"
      "nu"
    ];
    cargoTargets = [
      [
        "-p"
        "nu"
      ]
    ];
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };
in
  workspace.binaries.nu.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit workspace;
      }
      // lib.optionalAttrs (updateScriptWriter != null && repoPackages ? rebase-patches) {
        updateScript =
          ix.mkForkUpdater {
            writeNushellApplication = updateScriptWriter;
            inherit nix;
            rebasePatches = repoPackages.rebase-patches;
          } {
            name = "nushell";
            input = "nushell-src";
          };
      };

    meta =
      (old.meta or {})
      // {
        description = "Nushell with index's xattr-aware ls patch";
        homepage = "https://github.com/nushell/nushell";
        license = lib.licenses.mit;
        mainProgram = "nu";
        platforms = lib.platforms.unix;
      };
  })
