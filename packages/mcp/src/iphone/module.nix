{
  bundledSource,
  importTest,
  lib,
  pkgs,
  testsRoot,
}: let
  # The `iphone` helper source, bundled like `screen`/`vmkit`/`imessage` so every
  # session can `import iphone`. Pure Python: it shells out to the bundled
  # `pymobiledevice3` console script (resolved next to the interpreter at runtime)
  # and returns device data as polars frames and screenshots as PIL images.
  # Cross-platform (USB + a root `tunneld` are what it needs, not macOS), so it
  # builds and import-checks on Linux CI too.
  iphonePythonSource = bundledSource {
    name = "ix-mcp-iphone-python-source";
    path = ./.;
  };
  iphoneModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-iphone-python-module"
    {
      strictDeps = true;
      meta.description = "USB iOS device control (pymobiledevice3) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/iphone"
      mkdir -p "$site"
      cp -r ${iphonePythonSource}/iphone/. "$site/"
    ''
  );
  # The `iphone` helper imports in the real interpreter and exposes its surface.
  # Cross-platform: pulls in the vendored pymobiledevice3 CLI, so it also proves
  # that uv closure builds on Linux CI.
  iphoneBundled = importTest [iphoneModule] "iphone" "import iphone; print('iphone-ok', all(callable(getattr(iphone, n)) for n in ('devices', 'apps', 'screenshot', 'launch', 'start_tunneld', 'tap', 'swipe')))";
  # Device-free behaviour tests (exports, async signatures, explicit type hints,
  # CLI-path resolution, the sudo guard, the no-device error).
  iphoneTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.polars
    iphoneModule
  ]);
  iphoneTestSource = builtins.path {
    name = "ix-mcp-iphone-test";
    path = testsRoot + "/test_iphone.py";
  };
  typeHintSupport = builtins.path {
    name = "ix-mcp-iphone-type-hint-support";
    path = testsRoot + "/type_hint_support.py";
  };
  iphoneTests =
    pkgs.runCommand "ix-mcp-iphone-tests"
    {
      nativeBuildInputs = [iphoneTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${iphoneTestSource} "$TMPDIR/test_iphone.py"
      cp ${typeHintSupport} "$TMPDIR/type_hint_support.py"
      ${lib.getExe iphoneTestPython} -m pytest "$TMPDIR/test_iphone.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp iphone tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = iphoneModule;
  tests = {
    inherit
      iphoneBundled
      iphoneTests
      ;
  };
}
