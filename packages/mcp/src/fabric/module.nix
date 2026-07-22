{
  bundledSource,
  fleetModule,
  importTest,
  ixNotebookMcpModule,
  lib,
  pkgs,
  viewModule,
  weaveModule,
}: let
  # Call-first delegation on the weave journal (index#3191, #3192): `await
  # fabric.run(fn, *args)` executes fn on this node -- or, with node='<host>',
  # on that fleet node's runner actor over Ray, env-handshake-checked at
  # submit -- with the ask/started/terminal facts recorded via the bundled
  # weave client, and `fabric.claude` opens self-recording, interruptible
  # Claude Agent SDK sessions. Pure Python over the bundled weave +
  # claude-agent-sdk + ray (+ fleet for cluster discovery).
  fabricPythonSource = bundledSource {
    name = "ix-mcp-fabric-python-source";
    path = ./.;
  };
  fabricModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fabric-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [
        fleetModule
        ixNotebookMcpModule
        viewModule
        weaveModule
      ];
      meta.description = "Call-first fabric delegation bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/fabric"
      mkdir -p "$site"
      cp -r ${fabricPythonSource}/fabric/. "$site/"
    ''
  );
  fabricBundled = importTest [fabricModule] "fabric" "import fabric, asyncio; print('fabric-ok', asyncio.iscoroutinefunction(fabric.run), asyncio.iscoroutinefunction(fabric.claude.session), fabric.__version__)";
  # Network-free tests for the call-first fabric (index#3191, #3192, #3193): the run
  # record contract against an httpx.MockTransport weave double (ask facts at
  # submit with state strictly last, started/terminal facts from the worker
  # side, a fn that raises before its first line still leaving ask + failed),
  # claude.session's CAS-pointer turn facts plus both interrupt paths (handle
  # and journal fact) converging on the SDK interrupt as state=interrupted,
  # and the remote-placement submit contract with fakes (env handshake, host
  # label existence, zero-restart runner policy, cloudpickle payload round
  # trip through the real ray.cloudpickle, workspace materialization against
  # a local git fixture). Live-cluster behavior is validated manually
  # (index#3192 PR body); the sandbox proves everything submit-side.
  fabricTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.claude-agent-sdk
    ps.ray
    # fabric.activity.frame returns a polars frame.
    ps.polars
    fabricModule
    weaveModule
  ]);
  fabricTestSource = builtins.path {
    name = "ix-mcp-fabric-test";
    path = ./test_fabric.py;
  };
  # The weave client's unit tests share this env (httpx.MockTransport fakes);
  # no other derivation runs them. The file is weave's, riding its module drv
  # as passthru (src/weave/module.nix).
  weaveTestSource = weaveModule.ixTestSource;
  # Shared spool-teardown fixture, copied in as conftest.py so both
  # test_fabric.py and test_weave.py pick it up (src/weave/conftest.py).
  weaveConftestSource = weaveModule.ixConftestSource;
  fabricTests =
    pkgs.runCommand "ix-mcp-fabric-tests"
    {
      nativeBuildInputs = [
        fabricTestPython
        # The workspace tests build and clone a local git fixture.
        pkgs.git
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${fabricTestSource} "$TMPDIR/test_fabric.py"
      cp ${weaveTestSource} "$TMPDIR/test_weave.py"
      cp ${weaveConftestSource} "$TMPDIR/conftest.py"
      ${lib.getExe fabricTestPython} -m pytest "$TMPDIR/test_fabric.py" "$TMPDIR/test_weave.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp fabric tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = fabricModule;
  tests = {
    inherit
      fabricBundled
      fabricTests
      ;
  };
}
