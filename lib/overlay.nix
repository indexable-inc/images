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
# astlog-ignore: keep-overrides-composable
let
  # curl 8.21.0 started consuming the public curl_multi_wakeup() eventfd from
  # curl_multi_perform(). That loses a wakeup for callers which perform before
  # polling (notably Nix's file-transfer worker, which can then sleep for its
  # full 10-second idle timeout). Upstream fixed the regression by giving the
  # threaded resolver a separate internal wakeup pair:
  # https://github.com/curl/curl/issues/22272
  #
  # Keep the patch here until nixpkgs ships a curl release containing
  # 009fd378e8f01c97ebe67a14a41a06d56430f3df. The version assertion makes a
  # nixpkgs curl bump fail visibly instead of silently carrying a stale patch.
  curl = assert lib.assertMsg (prev.curl.version == "8.21.0")
  "remove the curl wakeup patch: expected nixpkgs curl 8.21.0, got ${prev.curl.version}";
    prev.curl.overrideAttrs (old: {
      patches =
        (old.patches or [])
        ++ [
          (prev.fetchurl {
            name = "curl-8.21.0-fix-multi-wakeup.patch";
            url = "https://github.com/curl/curl/commit/009fd378e8f01c97ebe67a14a41a06d56430f3df.patch";
            hash = "sha256-RMFcifj9jDaWY5jNBGqQc2NUoXb3+mHR/1ubrYjpHvc=";
          })
        ];
    });

  # gunicorn 26.0.0's TestASGIIntegration fixture binds a port
  # (`s.bind(('127.0.0.1', 0))`). The Darwin build sandbox has no loopback
  # unless the derivation opts in, so the fixture errors at setup and the build
  # fails. Linux sandboxes do provide loopback, which is why nixpkgs does not
  # see this: its recipe already disables two sibling integration tests for the
  # same class of failure ("failure while starting a gunicorn instance") but
  # never set the sandbox flag. With the flag the suite runs in full, 1874
  # passed and 350 skipped, so the tests are exercised rather than skipped.
  #
  # gunicorn is not wanted directly; it reaches the closure as a test input of
  # aiohttp, which the python env pulls in.
  #
  # Darwin-only so the Linux gunicorn keeps matching cache.nixos.org. Drop this
  # once nixpkgs sets __darwinAllowLocalNetworking on gunicorn itself.
  pythonPackagesExtensions =
    prev.pythonPackagesExtensions
    ++ lib.optional prev.stdenv.hostPlatform.isDarwin (_pyfinal: pyprev: {
      gunicorn = assert lib.assertMsg (pyprev.gunicorn.version == "26.0.0") ''
        remove the gunicorn loopback flag: expected nixpkgs gunicorn 26.0.0, got
        ${pyprev.gunicorn.version}. Recheck whether TestASGIIntegration still
        needs __darwinAllowLocalNetworking.'';
        pyprev.gunicorn.overridePythonAttrs (_: {
          __darwinAllowLocalNetworking = true;
        });

      # dunamai's test__version__from_git__with_annotated_tags commits to a
      # scratch repo, then asserts the commit timestamp is within one minute of
      # `now`. That one minute is a wall-clock budget for the test's own setup,
      # and a busy machine loses it: the suite takes over three minutes under
      # load here, and the commit was 72 seconds old by the time the assertion
      # ran (11:20:59 against a now of 11:22:11). 55 other cases passed.
      #
      # Widen the window rather than disable the case. dunamai read the
      # timestamp correctly, so only the freshness bound was wrong, and deleting
      # the test would drop real coverage of the timestamp plumbing.
      # `--replace-fail` makes an upstream rewrite of the assertion break loudly
      # here instead of silently no longer applying.
      dunamai = assert lib.assertMsg (pyprev.dunamai.version == "1.25.0") ''
        recheck the dunamai test timing patch: expected nixpkgs dunamai 1.25.0,
        got ${pyprev.dunamai.version}.'';
        pyprev.dunamai.overridePythonAttrs (old: {
          postPatch =
            (old.postPatch or "")
            + ''
              substituteInPlace tests/integration/test_dunamai.py \
                --replace-fail "delta = dt.timedelta(minutes=1)" "delta = dt.timedelta(minutes=30)"
            '';
        });
    });

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
    inherit curl pythonPackagesExtensions;

    # Default Temurin JRE for repo-owned package sets. The major lives in
    # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
    # and exported NixOS modules.
    ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
  }
