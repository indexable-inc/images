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
    # Upstream nushell pins reedline to a git rev on its main branch; the key
    # must match the Cargo.lock source string exactly, rev included. Refresh
    # after a nushell-src bump when eval reports a missing hash (index#3723).
    outputHashes = {
      "git+https://github.com/nushell/reedline?branch=main#f776f5079e49d075c071660ae0f9b040b3ff909b" = "sha256-Gy9OQJ2oAaZvy0XZ4dTDXEJa8caVHEh2yS5PovA8oi8=";
    };
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
