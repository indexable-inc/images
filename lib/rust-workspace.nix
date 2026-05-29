{
  lib,
  paths,
  packageRegistry,
  cargoUnitFor,
}:
workspacePkgs:
let
  inherit (paths) root;
  rustPackageFiles =
    packagePath:
    lib.fileset.intersection (lib.fileset.gitTracked packagePath) (
      lib.fileset.unions [
        (packagePath + "/Cargo.toml")
        (packagePath + "/src")
        (lib.fileset.maybeMissing (packagePath + "/benches"))
        (lib.fileset.maybeMissing (packagePath + "/build.rs"))
        (lib.fileset.maybeMissing (packagePath + "/tests"))
        (lib.fileset.maybeMissing (packagePath + "/templates"))
      ]
    );
  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.intersection (lib.fileset.gitTracked root) (
      lib.fileset.unions (
        [
          (root + "/Cargo.toml")
          (root + "/Cargo.lock")
          (rustPackageFiles (paths.modules + "/services/resource-monitor/stats-writer"))
        ]
        ++ map (entry: rustPackageFiles entry.path) packageRegistry.rustWorkspaceEntries
      )
    );
  };
  cargoLock = root + "/Cargo.lock";
  # One workspace-wide unit graph for every repo-owned Rust crate. Each
  # crate's `default.nix` picks its binary and test targets out of this
  # via `ix.cargoUnit.selectBinaryWithTests`, so the unit graph + vendor
  # closure get generated once instead of per crate. `nix-cargo-unit`
  # itself stays on the bootstrap path (it's what builds this graph).
  units = (cargoUnitFor workspacePkgs).buildWorkspace {
    pname = "ix-rust-workspace";
    inherit src;
    cargoLock.lockFile = cargoLock;
    workspaceRoot = root;
    cargoArgs = [ "--workspace" ];
    cargoTargets = [
      [ "--workspace" ]
      [
        "--workspace"
        "--tests"
      ]
      [
        "--workspace"
        "--benches"
      ]
    ];
    cargoTargetNames = [
      "build"
      "test"
      "bench"
    ];
    packageTestInputs.tui = [ workspacePkgs.vim ];
    packageTestInputs.ix-mcp = [ workspacePkgs.python3 ];
    # Every policy check runs once across the whole workspace. Selected
    # package outputs expose these as explicit tests instead of making
    # downstream binary builds depend on unrelated workspace policy.
    policy = {
      denyUnusedCrateDependencies = true;
      cargoAudit.enable = true;
      cargoMachete.enable = true;
      clippy.enable = true;
    };
  };
in
{
  inherit
    root
    src
    cargoLock
    units
    ;
}
