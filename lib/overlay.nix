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

      # ipython's test_system_interrupt starts a subprocess that sleeps five
      # seconds and asserts a SIGINT interrupts it. On a loaded machine the
      # pexpect spawn behind it fails, and ipython's own handler then reads
      # `child` before assignment, so the case dies as
      # `UnboundLocalError: cannot access local variable 'child'` in
      # IPython/utils/_process_posix.py rather than as a timeout. 1592 other
      # cases passed and the suite took 12 minutes.
      #
      # No bound to widen here, unlike dunamai: the failure is an unbound-local
      # in a error path, so the case goes in the list nixpkgs already keeps for
      # exactly this ("timing sensitive": test_debug_magic_passes_through_
      # generators, test_nest_embed). It just has not reached this one.
      ipython = assert lib.assertMsg (pyprev.ipython.version == "9.14.0") ''
        recheck the ipython disabled test: expected nixpkgs ipython 9.14.0, got
        ${pyprev.ipython.version}.'';
        pyprev.ipython.overridePythonAttrs (old: {
          disabledTests = (old.disabledTests or []) ++ ["test_system_interrupt"];
        });

      # inquirer3's tests/acceptance drives a real terminal through pexpect. All
      # ten cases fail with `pexpect.exceptions.TIMEOUT` here while the other 147
      # pass, so the library works and only the terminal-driving harness does
      # not survive this sandbox.
      #
      # nixpkgs disables nothing in this recipe, meaning its own build passes and
      # we would never have run these tests at all if we were substituting. We
      # build it because the curl patch above detaches this tree from
      # cache.nixos.org, which is the wider cost of that patch: packages nixpkgs
      # ships prebuilt get compiled here, and their suites then run on macOS 27
      # under whatever load the machine has.
      inquirer3 = assert lib.assertMsg (pyprev.inquirer3.version == "0.6.1") ''
        recheck the inquirer3 disabled tests: expected nixpkgs inquirer3 0.6.1,
        got ${pyprev.inquirer3.version}.'';
        pyprev.inquirer3.overridePythonAttrs (old: {
          disabledTestPaths = (old.disabledTestPaths or []) ++ ["tests/acceptance"];
        });

      # uvloop's suite does not survive a loaded machine, and naming the cases
      # does not converge. First run: test_call_at in both TestBaseUV and
      # TestBaseAIO, which assert a timer fires within 70ms
      # (`assertLess(finished - started, 0.07)`), measured at 0.162 and 0.079.
      # Skipping those two produced a different failure on the next run,
      # test_process_delayed_stdio__not_paused__no_stdin, which races a subprocess
      # against its stdio. Both are latency budgets in an event loop, so each run
      # under load picks a different victim out of 442.
      #
      # So drop the suite for this package rather than grow a list forever. The
      # coverage is not ours in the first place: nixpkgs CI runs it and
      # cache.nixos.org ships the result, and we only compile uvloop here because
      # the curl patch above detached this tree from that cache. A suite that
      # reports a different failure each run produces no signal we would act on.
      uvloop = assert lib.assertMsg (pyprev.uvloop.version == "0.22.1") ''
        recheck skipping the uvloop suite: expected nixpkgs uvloop 0.22.1, got
        ${pyprev.uvloop.version}.'';
        pyprev.uvloop.overridePythonAttrs (_: {
          doCheck = false;
        });
    });

  # graphite2 pins several CTest cases to `TIMEOUT 3` in its tests/*/CMakeLists
  # (comparerenderer and examples). Three seconds does not survive a busy
  # machine: five of 91 were killed at 12 to 27 seconds while the other 86
  # passed. CTest's `--timeout` cannot help, because an explicit per-test TIMEOUT
  # property wins over the command-line default, so raise the property itself.
  #
  # Like the python fixes above, we only run these tests because the curl patch
  # detaches this tree from cache.nixos.org; nixpkgs ships graphite2 prebuilt.
  graphite2 = assert lib.assertMsg (prev.graphite2.version == "1.3.15") ''
    recheck the graphite2 test timeouts: expected nixpkgs graphite2 1.3.15, got
    ${prev.graphite2.version}.'';
    prev.graphite2.overrideAttrs (old: {
      postPatch =
        (old.postPatch or "")
        + ''
          find tests -name CMakeLists.txt -exec sed -i 's/TIMEOUT 3)/TIMEOUT 300)/g' {} +
        '';
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
    inherit curl graphite2 pythonPackagesExtensions;

    # Default Temurin JRE for repo-owned package sets. The major lives in
    # `lib/languages/jvm-defaults.nix`, shared with `ix.languages.{java,scala}`
    # and exported NixOS modules.
    ixDefaultJre = final."temurin-jre-bin-${import ./languages/jvm-defaults.nix}";
  }
