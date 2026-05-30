{
  lib,
  paths,
  packageRegistry,
  cargoUnitFor,
  ghostty,
  writeNushellApplication,
}:
workspacePkgs:
let
  inherit (paths) root;

  # libghostty-vt built for the workspace's package set. ix-vt-sys links this
  # dylib, so the unit graph needs both the build-script env (so the build
  # script emits the link directives) and a workspace-wide `-L` search path (a
  # build script's own link-search does not propagate to the final per-unit
  # link in this graph; see the alsa note below for the same shape). The dylib
  # dir is also a runtime input for the ix-vt tests, which dlopen it.
  libghosttyVt = (import ./libghostty-vt.nix { inherit lib writeNushellApplication; }) workspacePkgs {
    ghosttySource = ghostty;
  };
  ghosttyLibDir = "${libghosttyVt}/lib";
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
    packageTestInputs = {
      tui = [ workspacePkgs.vim ];
      ix-mcp = [ workspacePkgs.python3 ];
      # tap's integration tests drive the `tap` binary on a PTY and run `bash`
      # as the session child; the daemon resolves `bash` from PATH at runtime.
      tap = [ workspacePkgs.bash ];
      # ix-vt's tests dlopen the libghostty-vt dylib at runtime; make its lib
      # dir available so the loader resolves `@rpath`/`-l ghostty-vt`.
      ix-vt = [ libghosttyVt ];
    };
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
    env = {
      # ix-vt-sys's build script reads this to emit the libghostty-vt link
      # search path. Set workspace-wide; only ix-vt-sys reads it.
      IX_VT_GHOSTTY_LIB_DIR = ghosttyLibDir;
    }
    // lib.optionalAttrs workspacePkgs.stdenv.hostPlatform.isLinux {
      PKG_CONFIG_PATH = "${workspacePkgs.alsa-lib.dev}/lib/pkgconfig";
    };
    extraRustcArgs = [
      # The libghostty-vt search path for the final per-unit link. A build
      # script's `rustc-link-search` does not reach the final binary link in
      # this graph, so the directory is added directly here (same shape as the
      # alsa-lib path below).
      "-L"
      "native=${ghosttyLibDir}"
      # Embed an rpath to the libghostty-vt store path so the linked binaries
      # (the ix-vt test binaries cargo-unit executes, and any future consumer)
      # resolve `libghostty-vt.so` at runtime without `LD_LIBRARY_PATH`. The
      # `-L` above only covers link time. Harmless for crates that never load it
      # because the binary keeps no `DT_NEEDED` entry for the lib.
      "-C"
      "link-arg=-Wl,-rpath,${ghosttyLibDir}"
    ]
    ++ lib.optionals workspacePkgs.stdenv.hostPlatform.isLinux [
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
