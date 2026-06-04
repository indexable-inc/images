{
  lib,
  paths,
  packageRegistry,
  cargoUnitFor,
  buildSvelteSite,
  ghostty,
  writeNushellApplication,
  # Cross-compilation leaves, threaded in so `unitsFor { target }` can build a
  # second unit graph for a non-host triple without `workspace.nix` having
  # to reach back into the assembled `ix` surface.
  rustToolchainFor,
  appleSdkToolchain,
  macosSdk,
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
  libghosttyVt =
    (import ../build/libghostty-vt.nix { inherit lib writeNushellApplication; }) workspacePkgs
      {
        ghosttySource = ghostty;
      };
  ghosttyLibDir = "${libghosttyVt}/lib";

  # The dashboard's single-page UI (Svelte/Vite, one self-contained index.html).
  # `dashboard-core`'s build script embeds it at compile time via
  # `IX_DASHBOARD_SITE_HTML` below, so the generated bundle is built by nix
  # rather than committed to the repo. Only `dashboard-core` reads the env var,
  # the same shape as `IX_VT_GHOSTTY_LIB_DIR`.
  dashboardSiteRoot = root + "/packages/dashboard-core/site";
  dashboardSite = buildSvelteSite workspacePkgs {
    pname = "dashboard-site";
    version = "0.1.0";
    src = lib.fileset.toSource {
      root = dashboardSiteRoot;
      fileset = lib.fileset.gitTracked dashboardSiteRoot;
    };
    serve.enable = false;
    devServer = {
      name = "dashboard-site-dev";
      checkoutSubdir = "packages/dashboard-core/site";
    };
  };
  dashboardSiteHtml = "${dashboardSite}/share/dashboard-site/index.html";
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

  # `cargo` cfg-excludes platform-gated deps per target, so an Apple-Silicon or
  # Intel macOS unit graph never sees `alsa-sys`; gate the ALSA plumbing on the
  # *target* OS rather than the build host so a Linux→macOS cross build does not
  # drag Linux audio inputs into a Darwin graph.
  targetIsLinux =
    target:
    if target == null then workspacePkgs.stdenv.hostPlatform.isLinux else lib.hasInfix "-linux-" target;

  # `vmkit` links libkrun for its Linux-guest backend, a different libkrun per
  # host. nixpkgs only provides `libkrun-efi` when the *build host* is
  # aarch64-darwin (it is not cross-buildable from Linux), and classic `libkrun`
  # only on a Linux host, so gate on the build host, NOT the target: a
  # Linux->darwin cross build (the `cross-darwin-smoke` check) must never force
  # `workspacePkgs.libkrun-efi`, which would refuse to evaluate on the Linux host.
  # When neither gate holds, `vmkit`'s build script omits the link env, so the
  # crate compiles without the libkrun backend (see its `build.rs`/`linuxkrun.rs`).
  buildHostIsAarch64Darwin =
    workspacePkgs.stdenv.hostPlatform.isDarwin && workspacePkgs.stdenv.hostPlatform.isAarch64;
  buildHostIsLinux = workspacePkgs.stdenv.hostPlatform.isLinux;

  # macOS host: libkrun-efi lib dir + the OVMF firmware blob it embeds (the latter
  # lives in the libkrun source tree). `vmkit`'s build script embeds the firmware
  # via `KRUN_EFI_FIRMWARE` and links `-lkrun`; the search path/rpath are injected
  # below because a build script's link-search does not reach the final unit link.
  # Only referenced under `buildHostIsAarch64Darwin`, so non-darwin hosts never
  # force the (host-only) package.
  libkrunEfiLibDir = "${workspacePkgs.libkrun-efi}/lib";
  krunEfiFirmware = "${workspacePkgs.libkrun-efi.src}/edk2/KRUN_EFI.silent.fd";

  # Linux host: classic KVM libkrun (no firmware). It boots a rootfs over virtiofs
  # under its bundled libkrunfw kernel, so the core path needs no block/net
  # feature; GPU, block, and net are enabled for parity with the macOS path and so
  # `--gpu` (and future disk boots) work. nixpkgs installs the shared lib into
  # `lib64` and force-links `-lkrunfw` with an rpath, so libkrun.so resolves
  # libkrunfw itself at runtime: only libkrun's own lib dir must reach our binary's
  # rpath. Only referenced under `buildHostIsLinux`, so darwin hosts never force it.
  libkrunLinux = workspacePkgs.libkrun.override {
    withBlk = true;
    withNet = true;
    withGpu = true;
  };
  libkrunLinuxLibDir = "${libkrunLinux}/lib64";

  # The Apple cross toolchain (zig cc + macOS SDK), or null for host/musl/Linux
  # targets that build with the ordinary linker.
  appleToolchainFor =
    target:
    if target != null && lib.hasSuffix "-apple-darwin" target then
      appleSdkToolchain {
        appleSdk = macosSdk { pkgs = workspacePkgs; };
        inherit lib target;
        pkgs = workspacePkgs;
      }
    else
      null;

  # rust-overlay toolchain carrying the cross target's `rust-std`. The native
  # graph keeps `cargo-unit`'s default (nixpkgs cargo + rustc).
  crossRustToolchain =
    target:
    rustToolchainFor workspacePkgs {
      channel = "stable";
      version = "latest";
      targets = [ target ];
    };

  # One workspace-wide unit graph for every repo-owned Rust crate. Each
  # crate's `default.nix` picks its binary and test targets out of the native
  # graph via `ix.cargoUnit.selectBinaryWithTests`, so the unit graph + vendor
  # closure get generated once instead of per crate. `nix-cargo-unit` itself
  # stays on the bootstrap path (it's what builds this graph). `target != null`
  # produces a separate cross graph used only to emit binaries.
  mkUnits =
    {
      target ? null,
    }:
    let
      appleToolchain = appleToolchainFor target;
      isCross = target != null;
    in
    (cargoUnitFor workspacePkgs).buildWorkspace (
      {
        pname = "ix-rust-workspace${lib.optionalString isCross "-${target}"}";
        inherit src;
        cargoLock.lockFile = cargoLock;
        workspaceRoot = root;
        cargoArgs = [ "--workspace" ];
        # Cross test/bench binaries can't execute on the build host, so a cross
        # graph builds only the `--workspace` root set; the native graph keeps
        # the test and bench roots for `passthru.tests`.
        cargoTargets = [
          [ "--workspace" ]
        ]
        ++ lib.optionals (!isCross) [
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
        ]
        ++ lib.optionals (!isCross) [
          "test"
          "bench"
        ];
        packageTestInputs = {
          tui = [ workspacePkgs.vim ];
          # tap's integration tests drive the `tap` binary on a PTY and run `bash`
          # as the session child; the daemon resolves `bash` from PATH at runtime.
          tap = [ workspacePkgs.bash ];
          # ix-vt's tests dlopen the libghostty-vt dylib at runtime; make its lib
          # dir available so the loader resolves `@rpath`/`-l ghostty-vt`.
          ix-vt = [ libghosttyVt ];
        };
        # `rodio` (packages/minecraft/sound) pulls `cpal`/`alsa-sys`, whose build
        # script needs ALSA's pkg-config metadata to link `libasound` on Linux.
        #
        # `pkg-config` + `PKG_CONFIG_PATH` let `alsa-sys`'s build script find ALSA
        # and emit `link-lib=asound`. That `-lasound` propagates to the final
        # `minecraft-sound` link, but the build script's `link-search` path does
        # not, so the linker reports `cannot find -lasound`. Add ALSA's lib dir to
        # every unit's rustc link search directly so the final binary link resolves
        # it. Harmless for crates that never reference `libasound`.
        nativeBuildInputs =
          lib.optional (targetIsLinux target) workspacePkgs.pkg-config
          ++ lib.optionals (appleToolchain != null) appleToolchain.runtimeInputs;
        env = {
          # ix-vt-sys's build script reads this to emit the libghostty-vt link
          # search path. Set workspace-wide; only ix-vt-sys reads it.
          IX_VT_GHOSTTY_LIB_DIR = ghosttyLibDir;
          # dashboard-core's build script reads this to embed the dashboard page.
          # Set workspace-wide; only dashboard-core reads it.
          IX_DASHBOARD_SITE_HTML = dashboardSiteHtml;
        }
        // lib.optionalAttrs (targetIsLinux target) {
          PKG_CONFIG_PATH = "${workspacePkgs.alsa-lib.dev}/lib/pkgconfig";
        }
        // lib.optionalAttrs buildHostIsAarch64Darwin {
          # vmkit's build script forwards this to a compile-time env so
          # linuxkrun.rs can `include_bytes!` the OVMF firmware, and uses its
          # presence to enable the libkrun-efi backend. Only vmkit reads it.
          KRUN_EFI_FIRMWARE = krunEfiFirmware;
        }
        // lib.optionalAttrs (buildHostIsLinux && !isCross) {
          # On a Linux host, signal vmkit's build script to link classic libkrun
          # (KVM). No firmware: the bundled libkrunfw kernel boots the rootfs. Only
          # vmkit reads it. Skipped for cross graphs, whose link search below is the
          # host's libkrun (wrong arch for a cross target).
          VMKIT_LINK_LIBKRUN = "1";
        }
        // lib.optionalAttrs (appleToolchain != null) appleToolchain.env;
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
        ++ lib.optionals (targetIsLinux target) [
          "-L"
          "native=${workspacePkgs.alsa-lib}/lib"
        ]
        ++ lib.optionals buildHostIsAarch64Darwin [
          # vmkit links `-lkrun` (libkrun-efi). Its build script emits the `-l`, but
          # the search path and rpath must be added here because a build script's
          # link-search does not reach the final unit link (same shape as the
          # alsa-lib and libghostty-vt paths above). Harmless for crates that never
          # reference libkrun, which keep no load command for it.
          "-L"
          "native=${libkrunEfiLibDir}"
          "-C"
          "link-arg=-Wl,-rpath,${libkrunEfiLibDir}"
        ]
        ++ lib.optionals (buildHostIsLinux && !isCross) [
          # vmkit links `-lkrun` (classic libkrun) on a Linux host. Same rationale
          # as the libkrun-efi branch above; nixpkgs installs libkrun into `lib64`.
          "-L"
          "native=${libkrunLinuxLibDir}"
          "-C"
          "link-arg=-Wl,-rpath,${libkrunLinuxLibDir}"
        ];
        # The native graph runs every policy check once across the whole
        # workspace (selected package outputs expose these as explicit tests).
        # A cross graph is a pure build artifact, so it skips policy to avoid
        # re-running clippy/audit/machete that the native graph already covers.
        policy =
          if isCross then
            {
              denyUnusedCrateDependencies = false;
              cargoAudit.enable = false;
              cargoMachete.enable = false;
              clippy.enable = false;
            }
          else
            {
              denyUnusedCrateDependencies = true;
              cargoAudit.enable = true;
              # cargo-machete is redundant with the per-crate
              # unused_crate_dependencies (rustc) gate, which is compile-based and
              # more precise than machete's heuristic scan, and machete only ran
              # as one whole-workspace pass. Rely on the per-crate check instead.
              cargoMachete.enable = false;
              clippy.enable = true;
            };
      }
      // lib.optionalAttrs isCross {
        inherit target;
        rustToolchain = crossRustToolchain target;
        extraRustcArgsForPlatform =
          if appleToolchain != null then appleToolchain.rustcArgsForPlatform else (_platform: [ ]);
      }
    );

  units = mkUnits { };
in
{
  inherit
    root
    src
    cargoLock
    units
    dashboardSite
    ;

  /**
    Build a cross-compiled unit graph for a non-host `target` triple.

    `target` is a Rust target triple. `aarch64-apple-darwin` /
    `x86_64-apple-darwin` build through the zig + macOS SDK toolchain (see
    [`lib/darwin/apple-sdk-toolchain.nix`](lib/darwin/apple-sdk-toolchain.nix)); other triples
    (e.g. `x86_64-unknown-linux-musl`) build with the ordinary linker and only
    need a toolchain carrying the target `rust-std`. Returns the same shape as
    `units`; select a binary with `ix.cargoUnit.selectBinaryWithTests` or
    `workspace.binaries.<name>`.
  */
  unitsFor =
    {
      target,
    }:
    mkUnits { inherit target; };
}
