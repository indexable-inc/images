"""A tiny read-only web dashboard over the execution store.

Auto-started by the CLI. It serves one self-contained HTML page (a Svelte/Vite
app under ``packages/mcp/site``, built by nix to a single ``index.html`` and
pointed to via ``IX_MCP_DASHBOARD_HTML`` on the package wrapper, the same shape
as dashboard-core's ``IX_DASHBOARD_SITE_HTML``). The page is static; it pulls
the live execution log from ``/api/jobs``, ``/api/resources``, and the curated
``/api/cells`` presentation once a second,
so a human can watch every running "thing" and its output like a notebook.

The page diffs the DOM reactively (Svelte) instead of rebuilding it, so scroll
position and open panels survive each refresh. When the env var is unset (a bare
run outside nix), a small stub explains how to build the UI.
"""

from __future__ import annotations

import os
from pathlib import Path

from aiohttp import web

from . import store
from .config import Config

_STUB = (
    "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
    "<title>ix-mcp</title></head>"
    "<body style=\"font:14px ui-monospace,monospace;background:#0b0b0c;"
    "color:#e6e6e6;padding:2rem\">"
    "<p>The dashboard UI was not built. Build through nix "
    "(<code>nix build .#mcp</code>), which sets <code>IX_MCP_DASHBOARD_HTML</code> "
    "to the Vite output. The data API is live at "
    "<code>/api/jobs</code> and <code>/api/resources</code>.</p></body></html>"
)


def _load_page() -> str:
    """The built single-file UI, or a stub when it was not built into the env."""
    path = os.environ.get("IX_MCP_DASHBOARD_HTML")
    if path:
        try:
            return Path(path).read_text(encoding="utf-8")
        except OSError:
            pass
    return _STUB


# Read once at startup: the page is an immutable nix-store artifact for the life
# of the server, and the live data arrives over the API rather than the HTML.
_PAGE = _load_page()


async def start(config: Config) -> web.AppRunner:
    app = web.Application()
    conn = store.connect(config.store_path)

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=_PAGE, content_type="text/html")

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(store.recent(conn, limit=200))

    async def resources(_request: web.Request) -> web.Response:
        return web.json_response(store.live_resources(conn))

    async def cells(_request: web.Request) -> web.Response:
        return web.json_response(store.cells(conn))

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    app.router.add_get("/api/resources", resources)
    app.router.add_get("/api/cells", cells)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, config.host, config.dashboard_port)
    await site.start()
    return runner
