{
  ix,
  lib,
}: let
  # The nushell view carries upstream main plus the xattr-aware ls patch.
  source = ix.nushellSrc;

  workspace = ix.cargoUnit.buildWorkspace {
    pname = "nushell";
    src = source;
    workspaceRoot = source;
    cargoLock = source + "/Cargo.lock";
    # Upstream nushell pins reedline to a git rev on its main branch; the key
    # must match the Cargo.lock source string exactly, rev included. Refresh
    # after a view update when eval reports a missing hash (index#3723).
    outputHashes = {
      "git+https://github.com/nushell/reedline?branch=main#7eb9bf219456202052aaa976842e9e790b88ed85" = "sha256-OYn2cCEZMR6Q8n8e/fwzpFRh1/kvybetHg86mactsMY=";
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
