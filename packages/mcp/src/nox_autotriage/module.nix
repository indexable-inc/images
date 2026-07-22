{
  bundledSource,
  importTest,
  lib,
  linearModule,
  pkgs,
  testsRoot,
}: let
  # `nox_autotriage`: nox-aware adapter that converts a nox conformance report
  # into linear.triage Findings and files them to Linear.  Depends on
  # linearModule (for linear.triage).  Entry point: python -m nox_autotriage.
  noxAutotriagePythonSource = bundledSource {
    name = "ix-mcp-nox-autotriage-python-source";
    path = ./.;
  };
  noxAutotriageModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-nox-autotriage-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [linearModule];
      meta.description = "nox conformance -> Linear triage adapter bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/nox_autotriage"
      mkdir -p "$site"
      cp -r ${noxAutotriagePythonSource}/nox_autotriage/. "$site/"
    ''
  );
  noxAutotriageBundled = importTest [noxAutotriageModule] "nox-autotriage" "import nox_autotriage; print('nox-autotriage-ok', callable(nox_autotriage.findings_from_conformance))";
  noxAutotriageTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.pydantic
    linearModule
    noxAutotriageModule
  ]);
  noxAutotriageTestSource = builtins.path {
    name = "ix-mcp-nox-autotriage-test";
    path = testsRoot + "/test_nox_autotriage.py";
  };
  noxAutotriageTestFixtures = builtins.path {
    name = "ix-mcp-nox-autotriage-test-fixtures";
    path = testsRoot + "/fixtures";
  };
  # The stub Linear GraphQL server shared with linear's triage tests
  # (../linear/module.nix pins the same file; both resolve to one store path).
  linearTestSupport = builtins.path {
    name = "ix-mcp-linear-test-support";
    path = testsRoot + "/linear_test_support.py";
  };
  noxAutotriageTests =
    pkgs.runCommand "ix-mcp-nox-autotriage-tests"
    {
      nativeBuildInputs = [noxAutotriageTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      mkdir -p "$TMPDIR/fixtures"
      cp ${noxAutotriageTestSource} "$TMPDIR/test_nox_autotriage.py"
      cp ${linearTestSupport} "$TMPDIR/linear_test_support.py"
      cp -r ${noxAutotriageTestFixtures}/. "$TMPDIR/fixtures/"
      ${lib.getExe noxAutotriageTestPython} -m pytest "$TMPDIR/test_nox_autotriage.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp nox-autotriage tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = noxAutotriageModule;
  tests = {
    inherit
      noxAutotriageBundled
      noxAutotriageTests
      ;
  };
}
