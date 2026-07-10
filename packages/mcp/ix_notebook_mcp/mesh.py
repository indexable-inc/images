"""The tailnet mesh endpoint: ``GET /mesh`` on every live ix-mcp (index#1787).

Each ix-mcp answers ``GET /mesh`` with a small read-only JSON identity card --
hostname, pid, build version, start time, the labels of its named sessions, its
dashboard URL, and the kernel's working directory -- on the well-known mesh
port (:data:`ix_notebook_mcp.config.DEFAULT_MESH_PORT`), bound ONLY to this
machine's tailscale IP. The bundled ``mesh`` kernel module (``src/mesh``) is
the client side: it sweeps the tailnet's online peers and collects these
cards, so starting ix-mcp anywhere on the tailnet means joining one visible
mesh with zero configuration.

Serving is default-on but never load-bearing: no tailscale, a stopped backend,
a bind race, or ``IX_MCP_MESH=0`` each skip the endpoint with one stderr line
and the MCP comes up exactly as before. Session NAMES are the only per-session
data exposed; code, outputs, and store contents stay behind the data API's own
network boundary.
"""

from __future__ import annotations

import os
import socket
import sys
from collections.abc import Callable
from datetime import UTC, datetime

from aiohttp import web

from .config import Config, mesh_enabled, mesh_port, server_version


def build_app(
    cfg: Config,
    session_names: Callable[[], list[str]],
    started_at: str,
    dashboard_url: str,
) -> web.Application:
    """Assemble the one-route mesh app over an injected session-name source.

    Split from :func:`start` so tests can drive the route without binding a
    socket (mirrors ``dashboard.build_app``); ``session_names`` is a callable
    (``tools.session_names`` in production) so this module never reaches into
    another module's private state. ``dashboard_url`` is the URL the server
    actually advertises (``cli._run`` resolves it AFTER the hub-spawn decision,
    so a failed auto-dashboard hub cannot leave a dead pre-spawn URL on the
    card -- index#1789 review); it is injected rather than read from the
    ``IX_MCP_DASHBOARD_URL`` env, which is the pre-kernel value.
    """
    app = web.Application()

    async def mesh(_request: web.Request) -> web.Response:
        # Session names are read per request: clients name themselves at any
        # point in their lifetime and the card must reflect the live set.
        return web.json_response(
            {
                "host": socket.gethostname(),
                "pid": os.getpid(),
                "version": server_version(),
                "started_at": started_at,
                "sessions": session_names(),
                "dashboard_url": dashboard_url,
                "cwd": str(cfg.workdir),
            }
        )

    app.router.add_get("/mesh", mesh)
    return app


def _skip(reason: str) -> None:
    print(f"[ix-mcp] mesh endpoint disabled: {reason}", file=sys.stderr, flush=True)


async def start(
    cfg: Config,
    session_names: Callable[[], list[str]],
    dashboard_url: str,
) -> web.AppRunner | None:
    """Serve ``/mesh`` on the tailscale IP, or skip (returning ``None``).

    Every skip path logs exactly one stderr line and never raises: the mesh is
    a discovery nicety, and it must not be able to take the MCP down
    (index#1787).
    """
    if not mesh_enabled():
        _skip("IX_MCP_MESH=0")
        return None
    if not cfg.mesh_host:
        # No usable tailscale IPv4 (no binary, or backend not Running): there
        # is no tailnet to mesh over, and any wider bind (LAN/wildcard) would
        # serve the card beyond the trust boundary, so skip entirely.
        _skip("no tailscale IPv4 to bind (the mesh serves the tailnet only)")
        return None
    started_at = datetime.now(UTC).isoformat()
    runner = web.AppRunner(build_app(cfg, session_names, started_at, dashboard_url))
    await runner.setup()
    port = mesh_port()
    try:
        await web.TCPSite(runner, cfg.mesh_host, port).start()
    except OSError as error:
        # A held port (a second ix-mcp on this box: the first keeps
        # advertising, which is correct for a per-machine well-known port) or
        # a tailscale-went-down race between IP resolution and the bind.
        await runner.cleanup()
        _skip(f"cannot bind {cfg.mesh_host}:{port} ({error})")
        return None
    print(f"[ix-mcp] mesh: http://{cfg.mesh_host}:{port}/mesh", file=sys.stderr, flush=True)
    return runner
