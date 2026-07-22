{
  bundledSource,
  bundledTestPython,
  ixNotebookMcpModule,
  lib,
  pkgs,
  testsRoot,
}: let
  # Polars-returning SSH fan-out source: `import fleet`, then `await fleet.scan`
  # runs a command on many hosts in parallel (asyncssh + a bounded semaphore on
  # the shared loop) and combines per-host stdout into one DataFrame via
  # `pl.concat(how="diagonal_relaxed")`. Pure Python over the bundled asyncssh +
  # polars; cross-platform, so every session can `import fleet`.
  fleetPythonSource = bundledSource {
    name = "ix-mcp-fleet-python-source";
    path = ./.;
  };
  fleetModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fleet-python-module"
    {
      strictDeps = true;
      meta.description = "Polars-returning SSH fan-out source bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/fleet"
      mkdir -p "$site"
      cp -r ${fleetPythonSource}/fleet/. "$site/"
    ''
  );
  # The fleet module: a mocked asyncssh connection drives the fan-out so the test
  # never depends on a reachable host or a running sshd. It asserts the contract
  # that matters: import works, the semaphore caps in-flight connections,
  # tag_host adds the column, diagonal_relaxed merges mismatched schemas, and
  # on_error="collect" survives a failing host (recording it in attrs). Pure
  # Python over the bundled asyncssh + polars, so the sandbox runs it.
  fleetTestPy = pkgs.writeText "ix-mcp-fleet-test.py" ''
    # python
    import asyncio
    import sys

    import polars as pl

    import fleet

    # --- Mock asyncssh so no network/sshd is needed. ---------------------------
    inflight = {"now": 0, "max": 0}

    class _Result:
        def __init__(self, stdout):
            self.stdout = stdout

    class _Conn:
        def __init__(self, host):
            self.host = host

        async def __aenter__(self):
            inflight["now"] += 1
            inflight["max"] = max(inflight["max"], inflight["now"])
            await asyncio.sleep(0.02)  # hold the slot so overlap is observable
            return self

        async def __aexit__(self, *exc):
            inflight["now"] -= 1
            return False

        async def run(self, command, encoding=None, check=True):
            # One host is configured to fail, to exercise on_error="collect".
            if self.host == "bad":
                raise RuntimeError("boom")
            # Two hosts return DIFFERENT schemas to exercise diagonal_relaxed:
            # one emits {a}, the other {b}.
            if self.host == "h1":
                return _Result(b'{"a": 1}\n')
            return _Result(b'{"b": 2}\n')

    def _connect(**opts):
        return _Conn(opts["host"])

    fleet.asyncssh.connect = _connect

    async def main():
        hosts = ["h1", "h2", "bad"]
        df = await fleet.scan(hosts, "noop", concurrency=2, on_error="collect")

        # tag_host added the host column, first.
        assert df.columns[0] == "host", df.columns
        # diagonal_relaxed unioned the {a} and {b} schemas.
        assert set(df.columns) == {"host", "a", "b"}, df.columns
        # Two good hosts -> two rows; the bad host did not abort the batch.
        assert df.height == 2, df.height
        # The failing host is recorded, not raised.
        fails = df.attrs["fleet_failures"]
        assert list(fails) == ["bad:22"], fails
        assert "boom" in fails["bad:22"], fails

        # Semaphore actually capped concurrency at 2 even with 3 hosts.
        assert inflight["max"] <= 2, inflight

        # on_error="raise" aggregates failures into FleetError.
        try:
            await fleet.scan(["bad"], "noop", on_error="raise")
        except fleet.FleetError as exc:
            assert "bad:22" in exc.failures, exc.failures
        else:
            raise AssertionError("expected FleetError")

        # read_text yields one row per line with host+line columns.
        class _Text(_Conn):
            async def run(self, command, encoding=None, check=True):
                return _Result(b"line one\nline two\n")
        fleet.asyncssh.connect = lambda **o: _Text(o["host"])
        txt = await fleet.read_text(["h1"], "/var/log/x")
        assert set(txt.columns) == {"host", "line"}, txt.columns
        assert txt.height == 2, txt.height

        # An all-empty fan-out returns an empty frame, never crashes.
        class _Empty(_Conn):
            async def run(self, command, encoding=None, check=True):
                return _Result(b"")
        fleet.asyncssh.connect = lambda **o: _Empty(o["host"])
        empty = await fleet.scan(["x"], "noop")
        assert isinstance(empty, pl.DataFrame) and empty.height == 0

        print("fleet-ok", fleet.__version__)

    asyncio.run(main())
  '';
  fleetSmokeTestPython = bundledTestPython [fleetModule];
  fleetSmoke =
    pkgs.runCommand "ix-mcp-fleet-smoke"
    {
      nativeBuildInputs = [fleetSmokeTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe fleetSmokeTestPython} ${fleetTestPy} >stdout 2>stderr || {
        echo "ix-mcp fleet smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^fleet-ok' stdout || {
        echo "ix-mcp fleet smoke did not confirm the fleet module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
  # The cluster surface (discovery merge, Ray submit return-shape, /api/exec
  # auth) with the two discovery sources, the Ray remote, and the kernel all
  # stubbed -- no live cluster or network. The env carries both `fleet` and
  # `ix_notebook_mcp`, so the script imports them with no PYTHONPATH.
  fleetClusterTestPython = bundledTestPython [
    fleetModule
    ixNotebookMcpModule
  ];
  fleetClusterSmoke =
    pkgs.runCommand "ix-mcp-fleet-cluster-smoke"
    {
      nativeBuildInputs = [fleetClusterTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe fleetClusterTestPython} ${testsRoot + "/fleet_cluster_check.py"} >stdout 2>stderr || {
        echo "ix-mcp fleet cluster smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^fleet-cluster-ok' stdout || {
        echo "ix-mcp fleet cluster smoke did not confirm the cluster surface:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = fleetModule;
  tests = {
    inherit
      fleetSmoke
      fleetClusterSmoke
      ;
  };
}
