"""Self-contained checks for the `fleet` cluster surface (run with mcpPython,
not pytest, matching the other bundled-module smokes). Prints `fleet-cluster-ok`
on success. No live Ray cluster or network: the two discovery sources, the Ray
remote, and the kernel are all stubbed by attribute assignment.

Defends three contracts:
- `fleet.nodes()` joins the tailscale view with the Ray view on the IPv4, and
  stays informative with only one source present.
- `fleet.submit` return shape follows `on` (fan-out -> keyed dict; single
  explicit target -> bare value), independent of how many nodes are alive.
- `/api/exec` is gated: disabled with neither token nor network trust, allowed
  on tailnet trust (a non-loopback bind), 401 on a wrong token, runs on the live
  kernel with the right one. `in_kernel` carries the token only when one is set.
"""

import asyncio
import pathlib
import tempfile

import fleet
from fleet import cluster


def check_nodes_merge() -> None:
    status = {
        "Self": {
            "DNSName": "hydra.ts.net.",
            "HostName": "hydra",
            "TailscaleIPs": ["100.0.0.1"],
            "Online": True,
        },
        "Peer": {
            "p": {"DNSName": "hc1.ts.net.", "TailscaleIPs": ["100.0.0.2"], "Online": True},
        },
    }
    ray_nodes = [
        {
            "NodeManagerAddress": "100.0.0.1",
            "Alive": True,
            "NodeID": "abc",
            "Resources": {"CPU": 8.0, "memory": 16_000_000_000},
        }
    ]

    async def fake_status() -> dict:
        return status

    cluster._tailscale_status = fake_status
    cluster._ray_nodes = lambda: ray_nodes
    df = asyncio.run(fleet.nodes())
    rows = {r["host"]: r for r in df.to_dicts()}

    assert rows["hydra.ts.net"]["self"] is True, rows["hydra.ts.net"]
    assert rows["hydra.ts.net"]["ray_alive"] is True, rows["hydra.ts.net"]
    assert rows["hydra.ts.net"]["cpu"] == 8.0, rows["hydra.ts.net"]
    assert rows["hydra.ts.net"]["memory_gb"] == 16.0, rows["hydra.ts.net"]
    # A tailnet peer with no Ray match still appears, Ray columns null.
    assert rows["hc1.ts.net"]["ray_alive"] is None, rows["hc1.ts.net"]
    assert rows["hc1.ts.net"]["tailscale_ip"] == "100.0.0.2", rows["hc1.ts.net"]


class _FakeRemote:
    """Stands in for ray.remote(fn).options(...); .remote() records the pinned
    node instead of scheduling anything."""

    def __init__(self, node_id: object) -> None:
        self._node_id = node_id

    def remote(self, *args: object, **kwargs: object) -> str:
        return "ref:" + str(self._node_id)


def check_submit_shape() -> None:
    cluster.connect = lambda *a, **k: None
    cluster._alive_ray_nodes = lambda: [
        {"NodeID": "n1", "NodeManagerHostname": "a", "NodeManagerAddress": "100.0.0.1"},
        {"NodeID": "n2", "NodeManagerHostname": "b", "NodeManagerAddress": "100.0.0.2"},
    ]
    cluster._remote = lambda fn, nid, opts: _FakeRemote(nid)

    assert fleet.submit(lambda: None, on="all") == {"a": "ref:n1", "b": "ref:n2"}
    assert fleet.submit(lambda: None, on=["a", "b"]) == {"a": "ref:n1", "b": "ref:n2"}
    assert fleet.submit(lambda: None, on="any") == "ref:None"
    assert fleet.submit(lambda: None, on="a") == "ref:n1"

    try:
        fleet.submit(lambda: None, on="nope")
    except cluster.ClusterError:
        pass
    else:
        raise AssertionError("expected ClusterError for an unknown target")


def check_in_kernel_tokenless() -> None:
    # in_kernel no longer hard-requires a token: a tailnet-trust cluster accepts
    # the call on membership alone. With no token and no reachable targets it
    # returns an empty frame rather than raising.
    import os

    os.environ.pop("IX_MCP_EXEC_TOKEN", None)
    os.environ.pop("IX_MCP_EXEC_TOKEN_FILE", None)

    async def no_targets(_on: object) -> list:
        return []

    cluster._http_targets = no_targets
    df = asyncio.run(fleet.in_kernel("all", "1+1"))
    assert df.height == 0, df


def check_exec_auth() -> None:
    from aiohttp.test_utils import TestClient, TestServer

    from ix_notebook_mcp import dashboard, kernel, store
    from ix_notebook_mcp.config import Config

    tmp = pathlib.Path(tempfile.mkdtemp())

    class _FakeKernel:
        async def python_exec(self, code: str, budget: float) -> tuple:
            return [], {"output": "", "result": "2", "error": None, "status": "ok"}

    kernel.current_kernel = _FakeKernel

    async def request(token: str | None, auth: str | None, payload: dict | None = None, *, trust: bool = False, host: str = "127.0.0.1") -> tuple:
        if payload is None:
            payload = {"code": "1+1"}
        conn = store.connect(tmp / "store.db")
        cfg = Config(
            workdir=tmp,
            store_path=tmp / "store.db",
            host=host,
            exec_token=token,
            exec_trust_network=trust,
        )
        app = dashboard.build_app(cfg, store.AsyncConn(cfg.store_path))
        async with TestClient(TestServer(app)) as client:
            headers = {"Authorization": auth} if auth else {}
            resp = await client.post("/api/exec", json=payload, headers=headers)
            body = await resp.json() if resp.status == 200 else None
            return resp.status, body

    status, _ = asyncio.run(request(None, None))
    assert status == 403, status  # disabled: neither token nor network trust
    # Trust the network only on a non-loopback bind; a loopback bind ignores it.
    status, _ = asyncio.run(request(None, None, trust=True, host="127.0.0.1"))
    assert status == 403, status
    status, body = asyncio.run(request(None, None, trust=True, host="100.0.0.5"))
    assert status == 200, (status, body)  # tailnet trust
    assert body["result"] == "2", (status, body)
    status, _ = asyncio.run(request("secret", "Bearer wrong"))
    assert status == 401, status  # a configured token is always required
    status, body = asyncio.run(request("secret", "Bearer secret"))
    assert status == 200, (status, body)
    assert body["result"] == "2", (status, body)
    # A non-numeric budget is a clean 400, not an unhandled 500.
    status, _ = asyncio.run(
        request("secret", "Bearer secret", {"code": "1", "budget": "abc"})
    )
    assert status == 400, status


def check_spark_dials_connect_url() -> None:
    # Mock pyspark so the smoke runs without a Spark cluster: assert fleet.spark
    # builds a Connect session against sc://<resolved-ip>:<SPARK_CONNECT_PORT>.
    import sys
    import types

    captured = {}

    class _Builder:
        def remote(self, url: str) -> "_Builder":
            captured["url"] = url
            return self

        def config(self, key: str, value: object) -> "_Builder":
            captured.setdefault("config", {})[key] = value
            return self

        def getOrCreate(self) -> str:
            return "spark-session"

    class _SparkSession:
        builder = _Builder()

    fake_sql = types.ModuleType("pyspark.sql")
    fake_sql.SparkSession = _SparkSession
    fake_pyspark = types.ModuleType("pyspark")
    fake_pyspark.sql = fake_sql
    sys.modules["pyspark"] = fake_pyspark
    sys.modules["pyspark.sql"] = fake_sql
    try:
        session = asyncio.run(fleet.spark(master="100.0.0.7"))
        assert session == "spark-session", session
        assert captured["url"] == "sc://100.0.0.7:15002", captured
        # No master and no env -> clean ClusterError, never a hang.
        import os

        os.environ.pop("IX_FLEET_SPARK_MASTER", None)
        try:
            asyncio.run(fleet.spark())
        except cluster.ClusterError:
            pass
        else:
            raise AssertionError("expected ClusterError without a master")
    finally:
        del sys.modules["pyspark"]
        del sys.modules["pyspark.sql"]


check_nodes_merge()
check_submit_shape()
check_exec_auth()
check_in_kernel_tokenless()
check_spark_dials_connect_url()
print("fleet-cluster-ok")
