{
  bundledSource,
  importTest,
  lib,
  pkgs,
  testsRoot,
}: let
  # Linear issue-tracker GraphQL client: `import linear`, then
  # `await linear.issue("ENG-123")` / `issue_update` / `issue_create` /
  # `project_create`. Pure Python over the already-bundled httpx; reads
  # LINEAR_API_KEY from the environment at call time.
  linearPythonSource = bundledSource {
    name = "ix-mcp-linear-python-source";
    path = ./.;
  };
  linearModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-linear-python-module"
    {
      strictDeps = true;
      meta.description = "Linear GraphQL client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/linear"
      mkdir -p "$site"
      cp -r ${linearPythonSource}/linear/. "$site/"
    ''
  );
  linearBundled = importTest [linearModule] "linear" "import linear; print('linear-ok', all(callable(getattr(linear, n)) for n in ('issue', 'issue_update', 'issue_create', 'issue_search', 'comment_create', 'project_create')), linear.__version__)";
  linearTriageTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.pydantic
    linearModule
  ]);
  linearTriageTestSource = builtins.path {
    name = "ix-mcp-linear-triage-test";
    path = testsRoot + "/test_linear_triage.py";
  };
  linearTestSupport = builtins.path {
    name = "ix-mcp-linear-test-support";
    path = testsRoot + "/linear_test_support.py";
  };
  linearTriageTests =
    pkgs.runCommand "ix-mcp-linear-triage-tests"
    {
      nativeBuildInputs = [linearTriageTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${linearTriageTestSource} "$TMPDIR/test_linear_triage.py"
      cp ${linearTestSupport} "$TMPDIR/linear_test_support.py"
      ${lib.getExe linearTriageTestPython} -m pytest "$TMPDIR/test_linear_triage.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp linear triage tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = linearModule;
  tests = {
    inherit
      linearBundled
      linearTriageTests
      ;
  };
}
