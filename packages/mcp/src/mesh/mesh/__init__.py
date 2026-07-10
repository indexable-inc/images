"""Tailnet mesh discovery: every live ix-mcp on the tailnet as polars frames.

Each ix-mcp serves a small identity card at ``GET /mesh`` on the well-known
mesh port, bound to its tailscale IP (``ix_notebook_mcp.mesh``, index#1787).
This module is the client side: sweep the tailnet's online peers from
``tailscale status --json``, fetch each card concurrently under a bounded
timeout, and return one polars row per responding server -- so "who else is
running ix-mcp, and what are their sessions working on" is one await with zero
configuration.

Usage::

    import mesh

    await mesh.peers()      # one row per live ix-mcp: host, ip, version, ...
    await mesh.sessions()   # flattened: one row per (host, session label)

Everything network-touching is ``async def`` because the kernel is one shared
event loop: the tailscale subprocess and the HTTP sweep must never block it.
Deliberately decoupled from ``fleet`` (its own tailscale helper, no import):
mesh discovery must work on any tailnet box, fleet or not (index#1787).
"""

from __future__ import annotations

import asyncio
import json
import os
import shutil
from pathlib import Path
from typing import Any

import polars as pl

# The mesh port constant and the tailnet-address gate are owned by the server
# package (the bind and the probes must agree on both); this module rides in
# the same bundled interpreter, so the import is satisfiable wherever
# `import mesh` itself is.
from ix_notebook_mcp.config import is_tailnet_ipv4, mesh_port

__version__ = "0.1.0"

__all__ = ["peers", "sessions"]

# Test seam: a stub script standing in for the real CLI (mirrors the
# resources_bridge IX_RESOURCES_BIN pattern), so tests never hit a real tailnet.
_TAILSCALE_BIN_ENV = "IX_MESH_TAILSCALE_BIN"

_PEERS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "host": pl.String,
    "ip": pl.String,
    "version": pl.String,
    "sessions": pl.List(pl.String),
    "dashboard_url": pl.String,
    "started_at": pl.String,
    "cwd": pl.String,
    "pid": pl.Int64,
}


def _exists(path: str) -> bool:
    """``Path.exists`` that treats an unstatable path as absent: the darwin nix
    build sandbox denies even ``stat`` on ``/usr`` (PermissionError, not
    False), and discovery must degrade to "no tailscale", never crash."""
    try:
        return Path(path).exists()
    except OSError:
        return False


def _find_tailscale() -> str | None:
    """The tailscale binary, honoring the test override, or None."""
    override = os.environ.get(_TAILSCALE_BIN_ENV)
    if override:
        return override if _exists(override) else None
    # macOS installs outside PATH-managed prefixes; same probe as the server's
    # cli._tailscale_status.
    return shutil.which("tailscale") or next(
        (p for p in ("/usr/local/bin/tailscale", "/usr/bin/tailscale") if _exists(p)),
        None,
    )


async def _tailscale_status() -> dict[str, Any] | None:
    """``tailscale status --json`` on the loop, or None when unavailable."""
    binary = await asyncio.to_thread(_find_tailscale)
    if not binary:
        return None
    try:
        proc = await asyncio.create_subprocess_exec(
            binary,
            "status",
            "--json",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )
        out, _ = await asyncio.wait_for(proc.communicate(), timeout=4)
        parsed: dict[str, Any] | None = json.loads(out) if out else None
        return parsed
    except Exception:
        return None


def _ipv4(ips: object) -> str | None:
    """The first probeable IPv4 in a TailscaleIPs list, or None.

    Accepts only tailscale's CGNAT range plus loopback: a malformed or spoofed
    status must not steer the sweep at a LAN or arbitrary address
    (index#1789 review, mirroring cli._tailscale_ip's bind gate). Loopback is
    allowed because it is this very machine (harmless to probe) and it is what
    the hermetic tests stand peers up on.
    """
    if not isinstance(ips, list):
        return None
    for ip in ips:
        if not isinstance(ip, str):
            continue
        if is_tailnet_ipv4(ip) or ip == "127.0.0.1":
            return ip
    return None


def _online_nodes(status: dict[str, Any]) -> list[tuple[str, str]]:
    """(host, ipv4) for Self plus every online peer.

    Self is always included: this very server's own card is part of the mesh
    view (and the one row a single-node tailnet still gets).
    """
    nodes: list[tuple[str, str]] = []
    entries: list[dict[str, Any]] = [status.get("Self") or {}]
    peer_map: dict[str, Any] = status.get("Peer") or {}
    if isinstance(peer_map, dict):
        entries += [p for p in peer_map.values() if isinstance(p, dict) and p.get("Online")]
    for entry in entries:
        ip = _ipv4(entry.get("TailscaleIPs"))
        if ip:
            host = str(entry.get("DNSName") or entry.get("HostName") or ip).rstrip(".")
            nodes.append((host, ip))
    return nodes


async def peers(timeout: float = 1.0) -> pl.DataFrame:
    """Every ix-mcp answering ``/mesh`` on the tailnet, one polars row each.

    Columns: ``host`` (the card's own hostname), ``ip`` (tailscale IPv4),
    ``version`` (build commit), ``sessions`` (named session labels),
    ``dashboard_url``, ``started_at``, ``cwd``, and ``pid``. Peers that are
    offline, not running ix-mcp, or slower than ``timeout`` seconds simply
    contribute no row -- discovery is a sweep, not a health check.

    Example::

        df = await mesh.peers()
        df.select("host", "version", "sessions")
    """
    # httpx is bundled but heavy; imported per call like fleet.in_kernel so
    # `import mesh` (preimported at kernel startup) stays light.
    import httpx

    status = await _tailscale_status()
    if not status:
        return pl.DataFrame(schema=_PEERS_SCHEMA)
    port = mesh_port()

    # verify=False: every card URL is plain `http://` inside the tailnet (the
    # transport itself is encrypted), so no TLS verification ever happens --
    # and building the default SSL context needs a CA bundle that hermetic
    # environments (the nix sandbox) do not provide.
    # trust_env=False: HTTP_PROXY/ALL_PROXY must never route these probes --
    # tailnet CGNAT addresses are unreachable through a proxy (every probe
    # would fail, and the sweep would leak tailnet IPs to it), and honoring a
    # NO_PROXY exemption per box is exactly the zero-config burden the mesh
    # exists to avoid (index#1789 review). Peers are dialed directly.
    async with httpx.AsyncClient(timeout=timeout, verify=False, trust_env=False) as client:  # noqa: S501 -- http-only client, see comment above

        async def one(host: str, ip: str) -> dict[str, Any] | None:
            try:
                resp = await client.get(f"http://{ip}:{port}/mesh")
                if resp.status_code != 200:
                    return None
                card = resp.json()
            except Exception:
                # A closed port, a timeout, or junk JSON all mean "no ix-mcp
                # here", which is a normal state for most tailnet peers.
                return None
            if not isinstance(card, dict):
                return None
            sessions_field = card.get("sessions")
            return {
                "host": str(card.get("host") or host),
                "ip": ip,
                "version": str(card.get("version") or ""),
                "sessions": [str(s) for s in sessions_field]
                if isinstance(sessions_field, list)
                else [],
                "dashboard_url": str(card.get("dashboard_url") or ""),
                "started_at": str(card.get("started_at") or ""),
                "cwd": str(card.get("cwd") or ""),
                "pid": int(card["pid"]) if isinstance(card.get("pid"), int) else None,
            }

        cards = await asyncio.gather(*(one(host, ip) for host, ip in _online_nodes(status)))

    rows = [card for card in cards if card is not None]
    if not rows:
        return pl.DataFrame(schema=_PEERS_SCHEMA)
    return pl.DataFrame(rows, schema=_PEERS_SCHEMA).sort("host")


async def sessions(timeout: float = 1.0) -> pl.DataFrame:
    """Every named session across the mesh: one row per (host, session).

    The flattened view of :func:`peers` for "what is everyone working on":
    columns ``host``, ``session``, ``dashboard_url``, ``ip``. A server with no
    named sessions contributes no rows here (it still shows in ``peers()``).
    """
    df = await peers(timeout=timeout)
    if df.is_empty():
        return pl.DataFrame(
            schema={
                "host": pl.String,
                "session": pl.String,
                "dashboard_url": pl.String,
                "ip": pl.String,
            }
        )
    return (
        df.explode("sessions")
        # A no-sessions server explodes to one null row; drop it, keeping the
        # contract "one row per actual named session".
        .drop_nulls("sessions")
        .rename({"sessions": "session"})
        .select("host", "session", "dashboard_url", "ip")
    )
