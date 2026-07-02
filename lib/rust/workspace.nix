{
  lib,
  paths,
  packageRegistry,
  cargoUnitFor,
  buildSvelteSite,
  buildLibghosttyVt,
  ghostty,
  writeBashApplication,
  # Cross-compilation leaves, threaded in so `unitsFor { target }` can build a
  # second unit graph for a non-host triple without `workspace.nix` having
  # to reach back into the assembled `ix` surface.
  rustToolchainFor,
  appleSdkToolchain,
  macosSdk,
  # The shared pins reader (lib/util/pins.nix), threaded down from
  # lib/default.nix so the libkrun-efi 1.19.3 pins load from the sibling
  # pins.json without a cross-directory `../` import (no-parent-path).
  pins,
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
  libghosttyVt = buildLibghosttyVt workspacePkgs {
    ghosttySource = ghostty;
  };
  ghosttyLibDir = "${libghosttyVt}/lib";

  # The dashboard's single-page UI (Svelte/Vite, one self-contained index.html).
  # `dashboard-core`'s build script embeds it at compile time via
  # `IX_DASHBOARD_SITE_HTML` below, so the generated bundle is built by nix
  # rather than committed to the repo. Only `dashboard-core` reads the env var,
  # the same shape as `IX_VT_GHOSTTY_LIB_DIR`.
  dashboardSiteRoot = root + "/packages/dashboard/dashboard-core/site";
  dashboardSite = buildSvelteSite workspacePkgs {
    sourceRoot = dashboardSiteRoot;
    serve.enable = false;
  };
  dashboardSiteHtml = "${dashboardSite}/share/dashboard-site/index.html";
  src =
    let
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
    in
    lib.fileset.toSource {
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
  #
  # nixpkgs pins libkrun-efi 1.18.0, whose vsock `from_tx_virtq_head` accepts
  # only the exact two-descriptor hdr+data TX layout; the combined or split
  # descriptor chains modern guest kernels emit are silently dropped mid
  # SOCK_STREAM (upstream containers/libkrun#535/#579), which desyncs the panes
  # guest->host frame stream (#1719). 1.19.3's packet.rs rewrite handles those
  # chains, so rebuild the same nixpkgs expression against the 1.19.3 source
  # until the nixpkgs pin catches up (nixpkgs master already carries this exact
  # bump; both hashes below match its pkgs/by-name/li/libkrun-efi). Version
  # deltas the override must carry: the upstream repo moved orgs
  # (containers -> libkrun), the guest init the vendored `init_blob` crate
  # embeds moved from `init/` to `src/init_blob/init/`, and the EFI firmware
  # moved from `edk2/` to `src/vmm/edk2/`.
  libkrunEfiSrcPin = pins.loadPin ./pins.json "libkrun-efi-src";
  libkrunEfiSrc = workspacePkgs.fetchFromGitHub {
    inherit (libkrunEfiSrcPin) owner repo hash;
    tag = "v${libkrunEfiSrcPin.version}";
  };
  # Same recipe as the `initBinary` in nixpkgs' libkrun-efi expression, rebuilt
  # here because the pinned 1.18.0 one compiles from `init/` in the old source.
  libkrunEfiInit = workspacePkgs.pkgsCross.aarch64-multiplatform.pkgsStatic.stdenv.mkDerivation {
    pname = "libkrun-init";
    inherit (libkrunEfiSrcPin) version;
    src = libkrunEfiSrc;

    dontConfigure = true;

    buildPhase = ''
      # shell
      runHook preBuild
      cd src/init_blob/init
      $CC -O2 -static -Wall -o init init.c dhcp.c
      runHook postBuild
    '';

    installPhase = ''
      # shell
      runHook preInstall
      install -D init $out/init
      runHook postInstall
    '';
  };
  libkrunEfi = workspacePkgs.libkrun-efi.overrideAttrs (old: {
    inherit (libkrunEfiSrcPin) version;
    src = libkrunEfiSrc;
    cargoDeps = workspacePkgs.rustPlatform.fetchCargoVendor {
      src = libkrunEfiSrc;
      inherit (pins.loadPin ./pins.json "libkrun-efi-cargo-vendor") hash;
    };
    env = (old.env or { }) // {
      KRUN_INIT_BINARY_PATH = "${libkrunEfiInit}/init";
    };
  });
  libkrunEfiLibDir = "${libkrunEfi}/lib";
  krunEfiFirmware = "${libkrunEfi.src}/src/vmm/edk2/KRUN_EFI.silent.fd";

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
      # `cargo` cfg-excludes platform-gated deps per target, so an Apple-Silicon
      # or Intel macOS unit graph never sees `alsa-sys`; gate the ALSA plumbing on
      # the *target* OS rather than the build host so a Linux→macOS cross build
      # does not drag Linux audio inputs into a Darwin graph.
      targetIsLinux =
        if target == null then workspacePkgs.stdenv.hostPlatform.isLinux else lib.hasInfix "-linux-" target;
      targetSystem =
        if target == null then
          workspacePkgs.stdenv.hostPlatform.system
        else if lib.hasSuffix "-apple-darwin" target then
          if lib.hasPrefix "aarch64-" target then "aarch64-darwin" else "x86_64-darwin"
        else if lib.hasPrefix "aarch64-" target then
          "aarch64-linux"
        else
          "x86_64-linux";
      excludedWorkspaceMembers = lib.filter (
        entry: !(builtins.elem entry (packageRegistry.rustWorkspaceEntriesFor targetSystem))
      ) packageRegistry.rustWorkspaceEntries;
      cargoWorkspaceExcludes = lib.concatMap (entry: [
        "--exclude"
        entry.id
      ]) excludedWorkspaceMembers;
      # A build script's `rustc-link-search` does not reach the final per-unit link
      # in this graph, so a linked native lib's directory is added to the link search
      # here directly, plus an rpath entry so the resulting binary resolves the shared
      # object at runtime without `LD_LIBRARY_PATH` (the `-L` alone only covers link
      # time). Harmless for crates that never reference the lib: they keep no
      # DT_NEEDED/load command for it.
      linkSearchWithRpath = dir: [
        "-L"
        "native=${dir}"
        "-C"
        "link-arg=-Wl,-rpath,${dir}"
      ];
      # The Apple cross toolchain (zig cc + macOS SDK), or null for host/musl/Linux
      # targets that build with the ordinary linker.
      appleToolchain =
        if target != null && lib.hasSuffix "-apple-darwin" target then
          appleSdkToolchain {
            appleSdk = macosSdk { pkgs = workspacePkgs; };
            inherit lib target writeBashApplication;
            pkgs = workspacePkgs;
          }
        else
          null;
      isCross = target != null;
      cargoUnit = cargoUnitFor workspacePkgs;
    in
    cargoUnit.buildWorkspace (
      {
        pname = "ix-rust-workspace${lib.optionalString isCross "-${target}"}";
        inherit src;
        cargoLock.lockFile = cargoLock;
        workspaceRoot = root;
        cargoArgs = [ "--workspace" ] ++ cargoWorkspaceExcludes;
        # Cross test/bench binaries can't execute on the build host, so a cross
        # graph builds only the `--workspace` root set; the native graph keeps
        # the test and bench roots for `passthru.tests`.
        cargoTargets = [
          ([ "--workspace" ] ++ cargoWorkspaceExcludes)
        ]
        ++ lib.optionals (!isCross) [
          (
            [
              "--workspace"
              "--tests"
            ]
            ++ cargoWorkspaceExcludes
          )
          (
            [
              "--workspace"
              "--benches"
            ]
            ++ cargoWorkspaceExcludes
          )
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
          # ix-vt's tests dlopen the libghostty-vt dylib at runtime; make its lib
          # dir available so the loader resolves `@rpath`/`-l ghostty-vt`.
          ix-vt = [ libghosttyVt ];
        };
        # `rodio` (packages/minecraft/minecraft/sound) pulls `cpal`/`alsa-sys`, whose build
        # script needs ALSA's pkg-config metadata to link `libasound` on Linux.
        #
        # `pkg-config` + `PKG_CONFIG_PATH` let `alsa-sys`'s build script find ALSA
        # and emit `link-lib=asound`. That `-lasound` propagates to the final
        # `minecraft-sound` link, but the build script's `link-search` path does
        # not, so the linker reports `cannot find -lasound`. Add ALSA's lib dir to
        # every unit's rustc link search directly so the final binary link resolves
        # it. Harmless for crates that never reference `libasound`.
        nativeBuildInputs =
          lib.optional targetIsLinux workspacePkgs.pkg-config
          ++ lib.optionals (appleToolchain != null) appleToolchain.runtimeInputs;
        env = {
          # ix-vt-sys's build script reads this to emit the libghostty-vt link
          # search path. Set workspace-wide; only ix-vt-sys reads it.
          IX_VT_GHOSTTY_LIB_DIR = ghosttyLibDir;
          # dashboard-core's build script reads this to embed the dashboard page.
          # Set workspace-wide; only dashboard-core reads it.
          IX_DASHBOARD_SITE_HTML = dashboardSiteHtml;
        }
        // lib.optionalAttrs targetIsLinux {
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
        # Build scripts emit native `-l` flags that propagate to downstream final
        # links, but their `rustc-link-search` paths do not cross cargo-unit's
        # per-unit derivation boundary. Keep native search/rpath args on final
        # link units only, so pure dependency rlibs remain independent of these
        # host native libraries.
        extraLinkRustcArgsForPlatform =
          _platform:
          linkSearchWithRpath ghosttyLibDir
          ++ lib.optionals targetIsLinux (
            [
              "-L"
              "native=${workspacePkgs.alsa-lib}/lib"
            ]
            # smithay's wayland_frontend (panes-compositor) links libxkbcommon:
            # the `-lxkbcommon` flag reaches the final link but, as with alsa
            # above, the emitting crate's link-search path does not. The rpath
            # keeps the guest binary loadable without LD_LIBRARY_PATH.
            ++ linkSearchWithRpath "${workspacePkgs.libxkbcommon}/lib"
          )
          ++ lib.optionals buildHostIsAarch64Darwin (linkSearchWithRpath libkrunEfiLibDir)
          ++ lib.optionals (buildHostIsLinux && !isCross) (linkSearchWithRpath libkrunLinuxLibDir);
        # The native graph runs every policy check once across the whole
        # workspace (selected package outputs expose these as explicit tests).
        # A cross graph is a pure build artifact, so it skips policy to avoid
        # re-running clippy/audit/machete that the native graph already covers.
        policy =
          if isCross then
            cargoUnit.policyPresets.pureBuild
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
        # rust-overlay toolchain carrying the cross target's `rust-std`. The
        # native graph keeps `cargo-unit`'s default (nixpkgs cargo + rustc).
        rustToolchain = rustToolchainFor workspacePkgs {
          channel = "stable";
          version = "latest";
          targets = [ target ];
        };
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
