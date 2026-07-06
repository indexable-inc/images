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
  # packages/agent/claude-code uses `ix.deepMerge.rhs`).
  ix,
}: final: prev:
# This `let` holds only the registry-iteration helpers (`overlayContext`,
# `buildOverlayPackage`); it hides no custom package. Every package is exposed as
# a real top-level overlay attr by the `genAttrs'` below (i.e. as
# `final.<attrName>`), so later overlays compose.
# astlog-ignore: keep-overrides-composable
let
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
    # TODO: re-add symphony-room-server. The room-server binary lives in the ix
    # monorepo (`ix#packages.x86_64-linux.room-server`); the ix<->index flake
    # cycle blocks sourcing it from there, so the old `symphony` input pin was
    # removed. Re-add a direct consumer before restoring this package.

    # Default Temurin JRE for repo-owned package sets. The major lives in
    # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
    # and exported NixOS modules.
    ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
  }
