{
  bundledSource,
  fleetModule,
  importTest,
  ixNotebookMcpModule,
  lib,
  pkgs,
  testsRoot,
}: let
  # Tailnet mesh discovery (index#1787): `await mesh.peers()` sweeps tailscale
  # peers for live ix-mcp `/mesh` endpoints (served by ix_notebook_mcp.mesh on
  # the well-known mesh port) and returns one polars row per responding server;
  # `mesh.sessions()` flattens to (host, session). Pure Python over the bundled
  # httpx + polars; cross-platform.
  meshPythonSource = bundledSource {
    name = "ix-mcp-mesh-python-source";
    path = ./.;
  };
  meshModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-mesh-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "Tailnet mesh discovery of live ix-mcp servers, bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/mesh"
      mkdir -p "$site"
      cp -r ${meshPythonSource}/mesh/. "$site/"
    ''
  );
  meshBundled = importTest [meshModule] "mesh" "import mesh, asyncio; print('mesh-ok', all(asyncio.iscoroutinefunction(getattr(mesh, n)) for n in ('peers', 'sessions')), mesh.__version__)";
  # Network-free tests for the tailnet auto-mesh (index#1787): the `/mesh`
  # route and its skip paths (IX_MCP_MESH=0, no tailscale IP, bind conflict),
  # the bundled `mesh` module's peer sweep against a STUB `tailscale` script
  # plus a real loopback server, and fleet.connect's zero-config Ray-head
  # probe against a fake GCS listener. asyncssh rides along because importing
  # `fleet` pulls it; `bash` backs the stub script's shebang. The session-label
  # tests import `ix_notebook_mcp.tools`, whose import chain needs the mcp SDK
  # (pydantic rides along) and nbformat (via `outputs`).
  meshTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.aiohttp
    ps.httpx
    ps.polars
    ps.asyncssh
    ps.mcp
    ps.nbformat
    ixNotebookMcpModule
    meshModule
    fleetModule
  ]);
  meshTestSource = builtins.path {
    name = "ix-mcp-mesh-test";
    path = testsRoot + "/test_mesh.py";
  };
  meshTests =
    pkgs.runCommand "ix-mcp-mesh-tests"
    {
      nativeBuildInputs = [
        meshTestPython
        pkgs.bash
      ];
      strictDeps = true;
      # The tests bind loopback sockets (a real mesh server + a fake GCS
      # listener); the darwin sandbox denies all binds without this. Linux
      # sandboxes already provide a private loopback, so it is a no-op there.
      __darwinAllowLocalNetworking = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${meshTestSource} "$TMPDIR/test_mesh.py"
      ${lib.getExe meshTestPython} -m pytest "$TMPDIR/test_mesh.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp mesh tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = meshModule;
  tests = {
    inherit
      meshBundled
      meshTests
      ;
  };
}
