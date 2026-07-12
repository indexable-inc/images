"""Network-free tests for the tailnet auto-mesh (index#1787).

Nothing here reaches a real tailnet, tailscale daemon, or Ray cluster:

* The **server side** (``ix_notebook_mcp.mesh``) is driven through aiohttp's
  in-process ``TestClient`` and, for the bind paths, real sockets on loopback
  only. Env overrides (``IX_MCP_MESH=0``, ``IX_MCP_MESH_PORT``), the
  no-tailscale skip, and the bind-conflict skip are all asserted to log one
  line and return ``None`` instead of raising.
* The **client side** (the bundled ``mesh`` module) gets a stub ``tailscale``
  script via ``IX_MESH_TAILSCALE_BIN`` (the resources_bridge stub pattern)
  whose JSON points at loopback, where a real mesh server (or nothing) runs.
* The **fleet probe** (``fleet.cluster``) gets a stub ``tailscale`` on PATH
  plus a fake Ray Client listener on an ephemeral loopback port selected
  through ``IX_FLEET_RAY_CLIENT_PORT``, proving ``connect()``'s zero-config
  resolution without importing Ray.
"""

from __future__ import annotations

import asyncio
import inspect
import json
import os
import shutil
import socket
import stat
import sys
from pathlib import Path
from typing import Any

import polars as pl
import pytest
from aiohttp import web
from aiohttp.test_utils import TestClient, TestServer

# Prefer the bundled packages (the nix check installs them into the
# interpreter); fall back to the source tree for a dev run.
_PKG_PARENT = Path(__file__).resolve().parents[1]
for _p in (_PKG_PARENT, _PKG_PARENT / "src" / "mesh", _PKG_PARENT / "src" / "fleet"):
    if str(_p) not in sys.path:
        sys.path.insert(0, str(_p))

import mesh as mesh_client
from fleet import cluster
from ix_notebook_mcp import cli
from ix_notebook_mcp import mesh as server_mesh
from ix_notebook_mcp.config import (
    DEFAULT_MESH_PORT,
    Config,
    build_stamp,
    is_tailnet_ipv4,
    mesh_enabled,
    mesh_port,
    server_version,
)


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _write_stub_tailscale(tmp_path: Path, status: dict[str, Any]) -> Path:
    """An executable stub ``tailscale`` that prints ``status`` for any args.

    Shebang resolved to bash's absolute path: the nix build sandbox has no
    /usr/bin/env, and bash is on PATH via the check's nativeBuildInputs.
    """
    bash = shutil.which("bash") or "/bin/bash"
    script = tmp_path / "tailscale"
    script.write_text(f"#!{bash}\ncat <<'IXEOF'\n{json.dumps(status)}\nIXEOF\n")
    script.chmod(script.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return script


def _status(
    self_ip: str | None = "127.0.0.1",
    peers: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    status: dict[str, Any] = {"BackendState": "Running", "Self": {}, "Peer": {}}
    if self_ip:
        status["Self"] = {"DNSName": "self.example.ts.net.", "TailscaleIPs": [self_ip]}
    for i, peer in enumerate(peers or []):
        status["Peer"][f"key{i}"] = peer
    return status


# ---------------------------------------------------------------------------
# Shape (mirrors the resources_bridge shape tests / the ruff ANN gate)
# ---------------------------------------------------------------------------


def test_all_names_exist() -> None:
    for name in mesh_client.__all__:
        assert hasattr(mesh_client, name), f"{name} in __all__ but missing from module"


def test_public_async_funcs_annotated() -> None:
    for name in ("peers", "sessions"):
        func = getattr(mesh_client, name)
        assert asyncio.iscoroutinefunction(func)
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty
        for pname, param in sig.parameters.items():
            assert param.annotation is not inspect.Parameter.empty, f"{name}({pname})"


# ---------------------------------------------------------------------------
# Config knobs
# ---------------------------------------------------------------------------


def test_mesh_port_default_and_override(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("IX_MCP_MESH_PORT", raising=False)
    assert mesh_port() == DEFAULT_MESH_PORT == 8798
    monkeypatch.setenv("IX_MCP_MESH_PORT", "9123")
    assert mesh_port() == 9123


def test_mesh_enabled_default_on(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("IX_MCP_MESH", raising=False)
    assert mesh_enabled()
    for value in ("0", "false", "no", "off", " OFF "):
        monkeypatch.setenv("IX_MCP_MESH", value)
        assert not mesh_enabled(), f"IX_MCP_MESH={value!r} must disable the mesh"
    monkeypatch.setenv("IX_MCP_MESH", "1")
    assert mesh_enabled()


def test_server_version_env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("IX_BUILD_REV", raising=False)
    assert server_version() == "dev"
    monkeypatch.setenv("IX_BUILD_REV", "abc123")
    assert server_version() == "abc123"


def test_build_stamp(monkeypatch: pytest.MonkeyPatch) -> None:
    """The stamp mirrors build-version's shape: short rev, commit date, age;
    an unknown epoch (unset or the 0 non-git sentinel) degrades to the bare
    short rev instead of rendering 1970."""
    rev = "7e42ccdb18827401226635"
    monkeypatch.setenv("IX_BUILD_REV", rev)
    monkeypatch.delenv("IX_BUILD_EPOCH", raising=False)
    assert build_stamp() == "7e42ccdb1882"
    monkeypatch.setenv("IX_BUILD_EPOCH", "0")
    assert build_stamp() == "7e42ccdb1882"
    monkeypatch.setenv("IX_BUILD_EPOCH", "not-a-number")
    assert build_stamp() == "7e42ccdb1882"
    # 1970-01-02T00:00:00Z, viewed just over two days later: the same fixture
    # as build-version's own stamp test, so the two implementations provably
    # render the identical line.
    monkeypatch.setenv("IX_BUILD_EPOCH", "86400")
    assert build_stamp(now=3 * 86400 + 1) == "7e42ccdb1882 (1970-01-02, 2 days ago)"


def test_is_tailnet_ipv4_gate() -> None:
    # The defense-in-depth gate on addresses taken from tailscale output
    # (index#1789 review, S1): only CGNAT 100.64.0.0/10 passes; wildcards,
    # LAN/loopback addresses, IPv6, and junk are all rejected by real parsing.
    assert is_tailnet_ipv4("100.64.0.0")
    assert is_tailnet_ipv4("100.115.233.43")
    assert is_tailnet_ipv4("100.127.255.255")
    assert not is_tailnet_ipv4("100.63.255.255")  # just below the range
    assert not is_tailnet_ipv4("100.128.0.0")  # just above the range
    assert not is_tailnet_ipv4("0.0.0.0")  # noqa: S104 -- asserting the wildcard is REJECTED
    assert not is_tailnet_ipv4("127.0.0.1")
    assert not is_tailnet_ipv4("192.168.1.10")
    assert not is_tailnet_ipv4("fd7a:115c:a1e0::1")
    assert not is_tailnet_ipv4("not-an-ip")
    assert not is_tailnet_ipv4("")


# ---------------------------------------------------------------------------
# The /mesh route (no socket: aiohttp TestClient)
# ---------------------------------------------------------------------------


def _fetch_card(app: web.Application) -> dict[str, Any]:
    async def go() -> dict[str, Any]:
        async with TestClient(TestServer(app)) as client:
            resp = await client.get("/mesh")
            assert resp.status == 200
            card: dict[str, Any] = await resp.json()
            return card

    return asyncio.run(go())


def test_mesh_card_fields(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv("IX_BUILD_REV", "deadbeef")
    cfg = Config(workdir=tmp_path)
    app = server_mesh.build_app(
        cfg,
        lambda: ["fix the build", "triage 1787"],
        "2026-07-03T00:00:00+00:00",
        "http://100.1.2.3:4567/",
    )
    card = _fetch_card(app)
    assert card["host"] == socket.gethostname()
    assert card["pid"] == os.getpid()
    assert card["version"] == "deadbeef"
    assert card["started_at"] == "2026-07-03T00:00:00+00:00"
    assert card["sessions"] == ["fix the build", "triage 1787"]
    assert card["dashboard_url"] == "http://100.1.2.3:4567/"
    assert card["cwd"] == str(tmp_path)


def test_mesh_card_dashboard_url_is_injected_not_env(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # The card must advertise the URL cli._run resolved AFTER the hub-spawn
    # decision, not the pre-kernel IX_MCP_DASHBOARD_URL env (which points at a
    # hub that may never have come up -- index#1789 review). The env decoy
    # here must lose to the injected value.
    monkeypatch.setenv("IX_MCP_DASHBOARD_URL", "http://100.0.0.1:1/dead-hub/")
    cfg = Config(workdir=tmp_path, advertised_host="100.9.9.9", dashboard_port=7777)
    app = server_mesh.build_app(cfg, list, "t", cfg.dashboard_url())
    assert _fetch_card(app)["dashboard_url"] == "http://100.9.9.9:7777/"


def test_mesh_card_sessions_are_live(tmp_path: Path) -> None:
    # The card reflects names set AFTER the server came up: names arrive at
    # any point in a client's lifetime, so the SAME app must serve both reads.
    names: list[str] = []
    app = server_mesh.build_app(Config(workdir=tmp_path), lambda: sorted(names), "t", "u")

    async def go() -> tuple[list[str], list[str]]:
        async with TestClient(TestServer(app)) as client:
            first = (await (await client.get("/mesh")).json())["sessions"]
            names.append("late namer")
            second = (await (await client.get("/mesh")).json())["sessions"]
            return first, second

    first, second = asyncio.run(go())
    assert first == []
    assert second == ["late namer"]


# ---------------------------------------------------------------------------
# session_names(): labels live exactly as long as their session
# ---------------------------------------------------------------------------


def test_session_labels_die_with_their_http_session(monkeypatch: pytest.MonkeyPatch) -> None:
    # A long-lived `serve --http` must not advertise a disconnected client's
    # label on /mesh forever (index#1789 review): labels are keyed weakly by
    # the live session object, so they vanish with it.
    import gc
    import weakref

    from ix_notebook_mcp import tools

    monkeypatch.setattr(tools, "_session_labels", weakref.WeakKeyDictionary())
    monkeypatch.setattr(tools, "_solo_session_name", None)

    class FakeSession:
        """Stands in for the mcp ServerSession (weakref-able, hashable)."""

    session = FakeSession()
    tools._session_labels[session] = "ephemeral client"
    assert tools.session_names() == ["ephemeral client"]
    del session
    gc.collect()
    assert tools.session_names() == []


def test_solo_session_label_via_set_and_names(monkeypatch: pytest.MonkeyPatch) -> None:
    # No config (an embedder) means one client and no session object to key
    # on: the label lands in the solo slot, gates naming, and is advertised.
    import weakref

    from ix_notebook_mcp import tools

    monkeypatch.setattr(tools, "_session_labels", weakref.WeakKeyDictionary())
    monkeypatch.setattr(tools, "_solo_session_name", None)
    assert tools._session_label(None) is None
    tools._set_session_label(None, "triage 1787")
    assert tools._session_label(None) == "triage 1787"
    assert tools.session_names() == ["triage 1787"]


# ---------------------------------------------------------------------------
# start(): the skip paths must log one line and never raise
# ---------------------------------------------------------------------------


def test_start_disabled_by_env(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("IX_MCP_MESH", "0")
    cfg = Config(workdir=tmp_path, mesh_host="127.0.0.1")
    assert asyncio.run(server_mesh.start(cfg, list, "u")) is None
    assert "mesh endpoint disabled" in capsys.readouterr().err


def test_start_skips_without_tailscale(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.delenv("IX_MCP_MESH", raising=False)
    cfg = Config(workdir=tmp_path, mesh_host=None)
    assert asyncio.run(server_mesh.start(cfg, list, "u")) is None
    assert "no tailscale" in capsys.readouterr().err


def test_start_serves_on_override_port(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # mesh_host is normally a tailscale IP; loopback here keeps the test
    # network-free while exercising the real bind + HTTP round trip.
    port = _free_port()
    monkeypatch.delenv("IX_MCP_MESH", raising=False)
    monkeypatch.setenv("IX_MCP_MESH_PORT", str(port))
    cfg = Config(workdir=tmp_path, mesh_host="127.0.0.1")

    async def go() -> dict[str, Any]:
        import aiohttp

        runner = await server_mesh.start(cfg, lambda: ["smoke"], "u")
        assert runner is not None
        try:
            async with (
                aiohttp.ClientSession() as session,
                session.get(f"http://127.0.0.1:{port}/mesh") as resp,
            ):
                assert resp.status == 200
                card: dict[str, Any] = await resp.json()
                return card
        finally:
            await runner.cleanup()

    assert asyncio.run(go())["sessions"] == ["smoke"]


def test_start_skips_on_bind_conflict(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.delenv("IX_MCP_MESH", raising=False)
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as holder:
        holder.bind(("127.0.0.1", 0))
        holder.listen(1)
        port = holder.getsockname()[1]
        monkeypatch.setenv("IX_MCP_MESH_PORT", str(port))
        cfg = Config(workdir=tmp_path, mesh_host="127.0.0.1")
        assert asyncio.run(server_mesh.start(cfg, list, "u")) is None
    assert "cannot bind" in capsys.readouterr().err


def test_wildcard_host_env_never_reaches_mesh_bind(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # The headline bind property (index#1789 review, C4): IX_MCP_HOST steers
    # the dashboard bind only; the mesh bind host comes solely from tailscale.
    # With IX_MCP_HOST=0.0.0.0 the tailscale-derived value is unchanged, and
    # with no tailscale at all the mesh SKIPS rather than widening to the env.
    monkeypatch.setenv("IX_MCP_HOST", "0.0.0.0")  # noqa: S104 -- asserting the wildcard CANNOT reach the bind
    _stub_tailscale_on_path(monkeypatch, tmp_path, _status(self_ip="100.99.1.1"))
    assert cli._tailscale_ip() == "100.99.1.1"

    # No tailscale: mesh_host resolves to None and start() must skip; the env
    # wildcard must not become a fallback bind host.
    monkeypatch.setenv("PATH", str(tmp_path / "empty"))
    if not _system_tailscale_present():
        assert cli._tailscale_ip() is None
    cfg = Config(workdir=tmp_path, mesh_host=None)
    assert asyncio.run(server_mesh.start(cfg, list, "u")) is None
    assert "no tailscale" in capsys.readouterr().err


def test_tailscale_ip_rejects_non_cgnat_addresses(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # A spoofed/malformed status handing out a wildcard, LAN, or IPv6 address
    # must not produce a bind host (index#1789 review, S1).
    bad = _status(self_ip=None)
    bad["Self"] = {"TailscaleIPs": ["0.0.0.0", "192.168.1.7", "fd7a:115c:a1e0::1", "junk"]}  # noqa: S104 -- hostile input under test, asserting it is rejected
    _stub_tailscale_on_path(monkeypatch, tmp_path, bad)
    assert cli._tailscale_ip() is None


# ---------------------------------------------------------------------------
# mesh.peers() / mesh.sessions(): stub tailscale + a real loopback server
# ---------------------------------------------------------------------------


def test_peers_empty_without_tailscale(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setenv(mesh_client._TAILSCALE_BIN_ENV, str(tmp_path / "missing-tailscale"))
    df = asyncio.run(mesh_client.peers())
    assert df.is_empty()
    assert set(df.columns) >= {"host", "ip", "version", "sessions", "dashboard_url"}


def test_peers_and_sessions_discover_live_server(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    port = _free_port()
    monkeypatch.delenv("IX_MCP_MESH", raising=False)
    monkeypatch.setenv("IX_MCP_MESH_PORT", str(port))
    monkeypatch.setenv("IX_BUILD_REV", "cafebabe")
    stub = _write_stub_tailscale(tmp_path, _status(self_ip="127.0.0.1"))
    monkeypatch.setenv(mesh_client._TAILSCALE_BIN_ENV, str(stub))
    cfg = Config(workdir=tmp_path, mesh_host="127.0.0.1")

    async def go() -> tuple[pl.DataFrame, pl.DataFrame]:
        runner = await server_mesh.start(cfg, lambda: ["alpha", "beta"], "http://127.0.0.1:9999/")
        assert runner is not None
        try:
            return await mesh_client.peers(), await mesh_client.sessions()
        finally:
            await runner.cleanup()

    peers_df, sessions_df = asyncio.run(go())
    assert peers_df.height == 1
    row = peers_df.to_dicts()[0]
    assert row["host"] == socket.gethostname()  # the card's hostname wins over DNSName
    assert row["ip"] == "127.0.0.1"
    assert row["version"] == "cafebabe"
    assert row["sessions"] == ["alpha", "beta"]
    assert row["dashboard_url"] == "http://127.0.0.1:9999/"
    assert row["pid"] == os.getpid()
    # sessions() flattens to one row per (host, session label).
    assert sessions_df.height == 2
    assert sessions_df["session"].to_list() == ["alpha", "beta"]
    assert set(sessions_df.columns) == {"host", "session", "dashboard_url", "ip"}


def test_peers_skips_offline_and_unresponsive(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # One offline peer (never probed) and one online peer with nothing
    # listening on the mesh port: both contribute no row, and neither raises.
    port = _free_port()
    monkeypatch.setenv("IX_MCP_MESH_PORT", str(port))
    status = _status(
        self_ip=None,
        peers=[
            {"HostName": "offline-box", "Online": False, "TailscaleIPs": ["127.0.0.1"]},
            {"HostName": "no-mcp-box", "Online": True, "TailscaleIPs": ["127.0.0.1"]},
        ],
    )
    stub = _write_stub_tailscale(tmp_path, status)
    monkeypatch.setenv(mesh_client._TAILSCALE_BIN_ENV, str(stub))
    df = asyncio.run(mesh_client.peers(timeout=0.5))
    assert df.is_empty()


def test_peers_never_probes_non_tailnet_addresses(monkeypatch: pytest.MonkeyPatch) -> None:
    # Spoofed/malformed peer addresses (wildcard, LAN, junk) are dropped by
    # the CGNAT gate before any probe (index#1789 review, S1); loopback is the
    # one non-CGNAT address allowed (it is this machine, and the test seam).
    assert mesh_client._ipv4(["0.0.0.0", "192.168.7.7", "junk"]) is None  # noqa: S104 -- hostile input under test, asserting it is rejected
    assert mesh_client._ipv4(["fd7a:115c:a1e0::1"]) is None
    assert mesh_client._ipv4(["100.86.202.115"]) == "100.86.202.115"
    assert mesh_client._ipv4(["0.0.0.0", "100.86.202.115"]) == "100.86.202.115"  # noqa: S104 -- hostile input under test, asserting it is skipped
    assert mesh_client._ipv4(["127.0.0.1"]) == "127.0.0.1"
    assert mesh_client._ipv4("not-a-list") is None


# ---------------------------------------------------------------------------
# fleet.connect() zero-config probe: stub tailscale on PATH + a fake Ray
# Client listener
# ---------------------------------------------------------------------------


def _stub_tailscale_on_path(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path, status: dict[str, Any]
) -> None:
    # cluster._find_tailscale resolves via shutil.which, so the stub goes on
    # PATH (prepended: bash and friends stay resolvable for the shebang).
    bin_dir = tmp_path / "stub-bin"
    bin_dir.mkdir(exist_ok=True)
    _write_stub_tailscale(bin_dir, status)
    monkeypatch.setenv("PATH", f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}")


def _server_peer(ip: str, *, online: bool = True, tags: list[str] | None = None) -> dict[str, Any]:
    return {
        "HostName": "fleet-node",
        "Online": online,
        "Tags": ["tag:server"] if tags is None else tags,
        "TailscaleIPs": [ip],
    }


def test_probe_finds_single_ray_client_head(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        # Backlog > 1: the listener never accept()s, and each probe's completed
        # handshake occupies a queue slot; both probes below must get through.
        listener.listen(8)
        port = listener.getsockname()[1]
        monkeypatch.setenv("IX_FLEET_RAY_CLIENT_PORT", str(port))
        _stub_tailscale_on_path(monkeypatch, tmp_path, _status(peers=[_server_peer("127.0.0.1")]))
        assert cluster._probe_ray_heads() == ["127.0.0.1"]
        # connect()'s resolution turns the one head into a Ray Client URL on
        # the SAME port it just probed (probe what you dial, index#1789 C1).
        target, note = cluster._resolve_auto_target()
        assert target == f"ray://127.0.0.1:{port}"
        assert note == ""


def test_probe_refuses_multiple_heads_loudly(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # TWO tag:server peers both answering on the probed port (index#1789 C5):
    # connect() must refuse to guess -- and it must HARD-FAIL, not fall back
    # to a private local Ray, which in exactly this ambiguous case would
    # silently compute on one laptop (index#1789 review). The error names
    # every hit so the operator can pick one. Both peers point at loopback
    # (the one address the sandbox can serve), so it is named twice.
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(8)
        monkeypatch.setenv("IX_FLEET_RAY_CLIENT_PORT", str(listener.getsockname()[1]))
        status = _status(peers=[_server_peer("127.0.0.1"), _server_peer("127.0.0.1")])
        _stub_tailscale_on_path(monkeypatch, tmp_path, status)
        assert cluster._probe_ray_heads() == ["127.0.0.1", "127.0.0.1"]
        with pytest.raises(RuntimeError) as excinfo:
            cluster._resolve_auto_target()
        note = str(excinfo.value)
        assert "2 Ray heads" in note
        assert "127.0.0.1, 127.0.0.1" in note  # every hit is named
        assert "IX_FLEET_RAY_ADDRESS" in note  # and the remedy


def test_probe_requires_server_tag_and_online(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        monkeypatch.setenv("IX_FLEET_RAY_CLIENT_PORT", str(listener.getsockname()[1]))
        status = _status(
            peers=[
                _server_peer("127.0.0.1", tags=[]),  # untagged: never probed
                _server_peer("127.0.0.1", online=False),  # offline: never probed
            ]
        )
        _stub_tailscale_on_path(monkeypatch, tmp_path, status)
        assert cluster._probe_ray_heads() == []


def test_probe_no_listener_yields_loud_note(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    # A tagged online peer with a CLOSED Ray Client port: the probe returns
    # nothing and the resolution hands connect() the why-line it prints on
    # fallback.
    monkeypatch.setenv("IX_FLEET_RAY_CLIENT_PORT", str(_free_port()))
    _stub_tailscale_on_path(monkeypatch, tmp_path, _status(peers=[_server_peer("127.0.0.1")]))
    target, note = cluster._resolve_auto_target(budget=0.5)
    assert target is None
    assert "no tag:server peer" in note


def _system_tailscale_present() -> bool:
    # Path.exists can raise PermissionError under the darwin nix sandbox
    # (stat on /usr is denied); that hermetic case is exactly "not present".
    for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale"):
        try:
            if Path(p).exists():
                return True
        except OSError:
            continue
    return False


@pytest.mark.skipif(
    _system_tailscale_present(),
    reason="a system tailscale shadows the empty-PATH case (dev box); the nix sandbox has neither",
)
def test_probe_without_tailscale(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # An empty PATH: shutil.which finds no tailscale, and the hardcoded
    # /usr/{local/,}bin fallbacks are absent in the build sandbox.
    monkeypatch.setenv("PATH", str(tmp_path / "empty"))
    assert cluster._probe_ray_heads() == []
