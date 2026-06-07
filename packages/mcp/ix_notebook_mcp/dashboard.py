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

import functools
import html
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


@functools.lru_cache(maxsize=512)
def _code_html(code: str) -> str:
    """A python snippet as self-contained highlighted HTML (inline monokai
    styles, no wrapping ``<pre>`` so the card controls layout). Every identifier
    token carries a ``data-ix-name`` so the dashboard can attach the value inlay
    and hover card to it; the join with values is by name, done in the browser
    against the job's ``bindings`` (kept out of here so this stays cache-keyed on
    the code text alone). Cached so each unique snippet is highlighted once, not
    on every one-second poll; falls back to empty (the card then shows the raw
    text) when pygments is unavailable."""
    if not code:
        return ""
    try:
        from pygments.lexers import PythonLexer
        from pygments.styles import get_style_by_name
        from pygments.token import Token

        style = get_style_by_name("monokai")
        parts: list[str] = []
        for token_type, value in PythonLexer().get_tokens(code):
            if not value:
                continue
            text = html.escape(value)
            css = _token_css(style, token_type)
            # Anchor only real identifiers (not builtins, operators, or the `@` of
            # a decorator) so the inlay/hover attaches to user namespace names. This
            # also tags attribute parts (`head` in `df.head`); the join is by name,
            # so they stay inert unless a same-named variable is live, an accepted
            # edge of name-keyed (vs position-keyed) matching.
            if token_type in Token.Name and token_type not in Token.Name.Builtin and value.isidentifier():
                attr = html.escape(value, quote=True)
                style_attr = f' style="{css}"' if css else ""
                parts.append(f'<span data-ix-name="{attr}"{style_attr}>{text}</span>')
            elif css:
                parts.append(f'<span style="{css}">{text}</span>')
            else:
                parts.append(text)
        return "".join(parts).strip("\n")
    except Exception:
        # Highlighting is cosmetic: a missing/old pygments must not break the API.
        return ""


def _token_css(style, token_type) -> str:
    """Inline CSS for one pygments token under ``style`` (the noclasses path,
    rebuilt here so we can also emit identifier anchors)."""
    spec = style.style_for_token(token_type)
    parts: list[str] = []
    if spec.get("color"):
        parts.append(f"color:#{spec['color']}")
    if spec.get("bold"):
        parts.append("font-weight:bold")
    if spec.get("italic"):
        parts.append("font-style:italic")
    if spec.get("underline"):
        parts.append("text-decoration:underline")
    return ";".join(parts)


async def start(config: Config) -> web.AppRunner:
    app = web.Application()
    conn = store.connect(config.store_path)

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=_PAGE, content_type="text/html")

    async def jobs(_request: web.Request) -> web.Response:
        rows = store.recent(conn, limit=200)
        for row in rows:
            # Highlight once per unique snippet (cached); the card renders it.
            row["code_html"] = _code_html(row.get("code") or "")
        return web.json_response(rows)

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
