{
  ix,
  lib,
}: let
  # The indexable-inc/nushell jj megamerge (nushell-src input): upstream main
  # plus the xattr-aware ls patch. The scheduled fork-sync rebases the fork
  # repo and floats the input.
  source = ix.nushellSrc;

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
;
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
