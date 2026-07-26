{
  lib,
  packageRegistry,
  buildIxRustTool,
  cargoUnitFor,
  clippy-src,
  rustWorkspaceFor,
  writeNushellApplication,
  writePythonApplication,
  # Curated cross-cutting helper surface (`lib`'s `sharedHelpers`), threaded in
  # as the `ix` arg so overlay-built packages can reach pure helpers like
  # `ix.deepMerge` exactly as flake-output packages do (lib/packages.nix binds
  # `ix` for the `packageSetFor` path). Without this, an overlay package that
  # takes an `ix` argument fails callPackage with a missing-arg error (e.g.
  # packages/claude-code uses `ix.deepMerge.rhs`).
  ix,
}: final: prev:
# This `let` holds only the registry-iteration helpers (`overlayContext`,
# `buildOverlayPackage`); it hides no custom package. Every package is exposed as
# a real top-level overlay attr by the `genAttrs'` below (i.e. as
# `final.<attrName>`), so later overlays compose.
#
# On what is deliberately NOT here. The curl 8.21.0 curl_multi_wakeup() patch
# used to be a global `curl` override in this file; it now lives in packages/nix
# on the `nix-store` component, the only thing that needs it. curl is a build
# input of git-minimal, which GHC reaches via sphinx -> pytest-xdist -> execnet
# -> hatch-vcs, and which rustc-bootstrap, cargo-bootstrap and the cargo vendor
# tooling reach too, so overriding `pkgs.curl` rehashed GHC, rustc, cargo and
# the whole python set and detached this tree from cache.nixos.org. Measured
# 2026-07-25 against nixpkgs e2587cae: arrow-cpp substitutes as a 28.5 MiB
# download and souffle as 2.8 MiB, and both were being compiled here for that
# one patch.
#
# Ten test-suite overrides went with it (graphite2, harfbuzz, gunicorn,
# requests-futures, watchfiles, dunamai, ipython, inquirer3, uvloop and the
# python valkey client). Each existed because the curl override forced a local
# build, which ran a suite nixpkgs had already run on its own builders, and
# those suites then failed on wall-clock budgets under load or on sandbox and
# macOS 27 differences. All ten substitute at this nixpkgs rev.
#
# The trap is worth naming, because it hides itself: an override changes the
# derivation hash, which forces the local build, which runs the suite, which is
# what the override works around. Adding one always looks like it fixed
# something. Before adding an override here, check whether the package
# substitutes without it (`nix build --dry-run nixpkgs#<pkg>` at this rev); if
# it does, the override is what will be creating the work. graphite2 and
# harfbuzz were also not Darwin-gated, so they detached the Linux tree for CI
# and the fleet, with pango and gtk under harfbuzz.
# astlog-ignore: keep-overrides-composable
let
  # valkey 9.1.0's integration/dual-channel-replication test ("Steady state
  # after dual channel sync ... Can't set new keys") fails deterministically on
  # ix dev/build machines: 5 consecutive from-source builds across two machine
  # classes (128-thread and 32-core EPYC), loaded, idle, sandboxed and
  # unsandboxed, 2026-07-25. valkey reaches ix closures only as a build-time
  # test fixture (python hishel's redis test hook, via ix-mcp), so its own
  # 4930-test suite is not an ix correctness gate. Checks off, like mdbook in
  # the ix repo, until nixpkgs carries a version whose suite passes here.
  valkey = prev.valkey.overrideAttrs (_: {doCheck = false;});

  # Read the target system from `prev`, not `final`: this overlay's attribute
  # *names* are computed by filtering the registry's `overlay` entries by
  # system (see `overlayEntriesFor`), so forcing the system through `final`
  # would require applying this overlay to know whether it defines `stdenv` --
  # a cycle. `prev` is the pre-overlay pkgs (same hostPlatform), so it breaks
  # the recursion. Without this, any registry entry with a non-null
  # `overlay.systems` triggers an infinite recursion (a `systems = null` entry
  # short-circuits before the system is ever forced, which is why it went
  # unnoticed).
  packageSystem = prev.stdenv.hostPlatform.system;
  overlayContext = entry: {
    inherit
      entry
      final
      prev
      lib
      buildIxRustTool
      clippy-src
      ;
    # Carry `pkgs` on the `ix` handle too (as `packageSetFor`'s `ixForPackages`
    # does), so overlay-built packages can read `ix.pkgs` instead of taking a
    # `pkgs` callPackage formal. Same value as the `pkgs` arg below (`final`).
    ix =
      ix
      // {
        pkgs = final;
        cargoUnit = cargoUnitFor final;
        rustWorkspace = rustWorkspaceFor final;
        patchedSrc = ix.patchedSrcFor final;
      };
    pkgs = final;
    inherit (entry) path;
    writeNushellApplication = writeNushellApplication final;
    writePythonApplication = writePythonApplication final;
  };
  buildOverlayPackage = entry:
  # This `let` only assembles the callPackage args for one registry entry and
  # returns the built package, which `genAttrs'` exposes as a top-level
  # `final.<attrName>`; it hides no package from later overlays.
  # astlog-ignore: keep-overrides-composable
  let
    context = overlayContext entry;
    autoArgs = final // context;
  in
    if entry.overlay ? build
    then entry.overlay.build context
    else lib.callPackageWith autoArgs entry.path {};
in
  lib.genAttrs' (packageRegistry.overlayEntriesFor packageSystem) (
    entry: lib.nameValuePair entry.overlay.attrName (buildOverlayPackage entry)
  )
  // {
    inherit valkey;

    # Default Temurin JRE for repo-owned package sets. The major lives in
    # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
    # and exported NixOS modules.
    ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
  }
