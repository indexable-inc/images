{
  bundledSource,
  bundledTestPythonWith,
  fontsConf,
  importTest,
  ixNotebookMcpModule,
  lib,
  pkgs,
  playwrightBrowsers,
  svelteBundleBin,
  testsRoot,
}: let
  # Svelte 5 components as live interactive resources: `import svelte`, then
  # `await svelte.component("Board.svelte", id=..., actions=...)` compiles via
  # the svelte-bundle CLI and registers the result, with the virtual `ix`
  # module (`data`/`act`/`replies`) wired to the resource event feed. Pure
  # Python; the compiler is the wrapped Node CLI above.
  sveltePythonSource = bundledSource {
    name = "ix-mcp-svelte-python-source";
    path = ./.;
  };
  svelteModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-svelte-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "Svelte 5 resource components bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/svelte"
      mkdir -p "$site"
      cp -r ${sveltePythonSource}/svelte/. "$site/"
    ''
  );
  # The whole Svelte resource path (packages/mcp/tests/test_svelte.py): the
  # nix-built svelte-bundle CLI compiles a Svelte 5 component, a real sandboxed
  # iframe renders the kernel-embedded state, `act` rides the real /api/input,
  # and the action_result re-renders the page. Same interpreter + browser needs
  # as inputBrowserSmoke, plus the CLI on IX_SVELTE_BUNDLE_BIN.
  svelteBundled = importTest [svelteModule] "svelte" "import svelte; print('svelte-ok', callable(svelte.bundle), callable(svelte.component))";
  svelteTestPython = bundledTestPythonWith (ps: [ps.pytest]) [svelteModule];
  svelteTestSource = builtins.path {
    name = "ix-mcp-svelte-test";
    path = testsRoot + "/test_svelte.py";
  };
  svelteTests =
    pkgs.runCommand "ix-mcp-svelte-tests"
    {
      nativeBuildInputs = [svelteTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      export PLAYWRIGHT_BROWSERS_PATH=${lib.escapeShellArg playwrightBrowsers}
      export FONTCONFIG_FILE=${fontsConf}
      export IX_SVELTE_BUNDLE_BIN=${lib.escapeShellArg (lib.getExe svelteBundleBin)}
      cp ${svelteTestSource} "$TMPDIR/test_svelte.py"
      ${lib.getExe svelteTestPython} -m pytest "$TMPDIR/test_svelte.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp svelte tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = svelteModule;
  tests = {
    inherit
      svelteBundled
      svelteTests
      ;
  };
}
