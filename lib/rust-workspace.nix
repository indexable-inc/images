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
    # `rodio` (packages/minecraft/sound) pulls `cpal`/`alsa-sys`, whose build
    # script needs ALSA's pkg-config metadata to link `libasound` on Linux.
    # Scoped to the whole workspace because the unit graph compiles every
    # member on every system; darwin uses CoreAudio and needs nothing extra.
    #
    # `pkg-config` + `PKG_CONFIG_PATH` let `alsa-sys`'s build script find ALSA
    # and emit `link-lib=asound`. That `-lasound` propagates to the final
    # `minecraft-sound` link, but the build script's `link-search` path does
    # not, so the linker reports `cannot find -lasound`. Add ALSA's lib dir to
    # every unit's rustc link search directly so the final binary link resolves
    # it. Harmless for crates that never reference `libasound`.
    nativeBuildInputs = lib.optional workspacePkgs.stdenv.hostPlatform.isLinux workspacePkgs.pkg-config;
    env = lib.optionalAttrs workspacePkgs.stdenv.hostPlatform.isLinux {
      PKG_CONFIG_PATH = "${workspacePkgs.alsa-lib.dev}/lib/pkgconfig";
    };
    extraRustcArgs = lib.optionals workspacePkgs.stdenv.hostPlatform.isLinux [
      "-L"
      "native=${workspacePkgs.alsa-lib}/lib"
    ];
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
