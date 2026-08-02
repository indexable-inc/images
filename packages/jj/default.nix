{
  ix,
  lib,
}: let
  # The indexable-inc/jj jj megamerge (jj-src input): upstream main plus the
  # submodule series. The input is pinned by rev and never floats under the
  # fork-sync cron; see the jj-src comment in flake.nix.
  source = ix.jjSrc;

  workspace = ix.cargoUnit.buildWorkspace {
    pname = "jj";
    src = source;
    workspaceRoot = source;
    cargoLock = source + "/Cargo.lock";
    # jj-cli is the CLI crate; its `jj` bin target is the only one outside the
    # test-fakes feature, and the rest of the workspace is libraries.
    cargoArgs = [
      "-p"
      "jj-cli"
    ];
    cargoTargets = [
      [
        "-p"
        "jj-cli"
      ]
    ];
    # The repo-owned quality gates are for crates we own; upstream jj answers
    # to its own CI.
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy.enable = false;
    };
  };
in
  workspace.binaries.jj.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit workspace;
      };
    meta =
      (old.meta or {})
      // {
        description = "Jujutsu with index's git submodule series";
        homepage = "https://github.com/jj-vcs/jj";
        license = lib.licenses.asl20;
        mainProgram = "jj";
        platforms = lib.platforms.unix;
      };
  })
