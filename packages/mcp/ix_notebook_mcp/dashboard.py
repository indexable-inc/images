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

from . import feed, store
from .config import Config

_STUB = (
    "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">"
    "<title>ix-mcp</title>"
    "<style>:root{color-scheme:dark light}"
    "body{font:14px ui-monospace,monospace;background:#0b0b0c;color:#e6e6e6;padding:2rem}"
    "@media(prefers-color-scheme:light){body{background:#fbfbfc;color:#1b1b1f}}</style>"
    "</head><body>"
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


@functools.lru_cache(maxsize=512)
def _code_html(code: str) -> str:
    """A python snippet as highlighted, *line-addressable* HTML.

    Each source line becomes one ``<span class="ix-line" data-line="N">`` (1-based,
    matching compiler/traceback line numbers) holding its highlighted tokens: one
    ``<span class="...">`` per token using pygments' standard short class names,
    with no inline colors and no wrapping ``<pre>`` (the card controls layout).
    The line spans are what let the dashboard point at a *line*: the live
    executing line while a job runs and the failing line on an error, plus a CSS
    line-number gutter (``style.css``). A token spanning lines (a triple-quoted
    string) is split so every piece sits inside its own line span.

    The class palette is themed in CSS (``_highlight_css``), so the same cached
    HTML reads correctly in both the dark and light dashboard. Every identifier
    token also carries a ``data-ix-name`` so the dashboard can attach the value
    hover card; the join with values is by name, done in the browser against the
    job's ``bindings`` (kept out of here so this stays cache-keyed on the code
    text alone). Cached so each unique snippet is highlighted once, not on every
    one-second poll; falls back to empty (the card then shows the raw text) when
    pygments is unavailable."""
    if not code:
        return ""
    try:
        from pygments.lexers import PythonLexer
        from pygments.token import Token

        lines: list[list[str]] = [[]]
        # stripnl=False: the default strips leading/trailing blank lines, which
        # would shift data-line off the real (traceback) line numbers.
        for token_type, value in PythonLexer(stripnl=False).get_tokens(code):
            if not value:
                continue
            cls = _token_class(token_type)
            cls_attr = f' class="{cls}"' if cls else ""
            # Anchor only real identifiers (not builtins, operators, or the `@` of
            # a decorator) so the inlay/hover attaches to user namespace names. This
            # also tags attribute parts (`head` in `df.head`); the join is by name,
            # so they stay inert unless a same-named variable is live, an accepted
            # edge of name-keyed (vs position-keyed) matching.
            named = (
                token_type in Token.Name
                and token_type not in Token.Name.Builtin
                and value.isidentifier()
            )
            for index, piece in enumerate(value.split("\n")):
                if index:
                    lines.append([])
                if not piece:
                    continue
                text = html.escape(piece)
                if named:
                    attr = html.escape(value, quote=True)
                    lines[-1].append(f'<span{cls_attr} data-ix-name="{attr}">{text}</span>')
                elif cls:
                    lines[-1].append(f"<span{cls_attr}>{text}</span>")
                else:
                    lines[-1].append(text)
        # Pygments guarantees a trailing newline; drop the trailing blank line(s)
        # it produces (mirrors the old `.strip("\n")`) without shifting numbering.
        while lines and not lines[-1]:
            lines.pop()
        return "".join(
            f'<span class="ix-line" data-line="{number}">{"".join(parts)}</span>'
            for number, parts in enumerate(lines, 1)
        )
    except Exception:
        # Highlighting is cosmetic: a missing/old pygments must not break the API.
        return ""


def _token_class(token_type) -> str:
    """The pygments standard short CSS class for a token (``k``, ``s``, ``nf``,
    ...), climbing to the nearest classified ancestor. Empty for plain text,
    which is then emitted without a span. Matches the class names in the
    stylesheet ``_highlight_css`` builds."""
    from pygments.token import STANDARD_TYPES

    ttype = token_type
    while ttype not in STANDARD_TYPES:
        ttype = ttype.parent
    return STANDARD_TYPES[ttype]


def _highlight_css() -> str:
    """Two scoped token palettes for the highlighted source: monokai for the dark
    dashboard (the default) and a light palette under ``prefers-color-scheme:
    light``. Both are scoped to ``.ix-code`` so they only touch injected source
    spans, and the chrome rules (background, line numbers, highlight line) are
    dropped so tokens inherit the dashboard's own ``--inset`` box. Empty when
    pygments is unavailable.

    The light block must override *every* class the dark block colors. monokai
    paints punctuation (``.p`` -- the parens, commas, dots) and several generic
    tokens near-white; the light style (xcode) never restyles those, so without
    an explicit override the dark white leaks into light mode and the punctuation
    is invisible white-on-white. For any class the light style omits we reset to
    the dashboard's own text color so it always reads."""
    try:
        from pygments.formatters import HtmlFormatter
    except Exception:
        return ""

    def token_rules(style_name: str) -> dict[str, str]:
        """Map each token selector (``.ix-code .<cls>``) to its declaration body,
        keeping only per-token rules (not the background/line-number/highlight
        chrome the formatter adds)."""
        defs = HtmlFormatter(style=style_name).get_style_defs(".ix-code")
        rules: dict[str, str] = {}
        for line in defs.splitlines():
            stripped = line.strip()
            if not stripped.startswith(".ix-code ."):
                continue
            if stripped.startswith(".ix-code .hll"):
                continue
            selector, _, rest = stripped.partition("{")
            rules[selector.strip()] = rest.split("}", 1)[0].strip()
        return rules

    dark = token_rules("monokai")
    light = token_rules("xcode")
    # Reset (not just recolor) any class the light palette omits, clearing the
    # dark weight/style too so nothing bleeds through.
    reset = "color: inherit; font-weight: normal; font-style: normal"
    dark_css = "\n".join(f"{sel} {{ {decl} }}" for sel, decl in dark.items())
    light_css = "\n".join(
        f"{sel} {{ {light.get(sel, reset)} }}" for sel in {**dark, **light}
    )
    return f"{dark_css}\n@media (prefers-color-scheme: light) {{\n{light_css}\n}}\n"


def _with_highlight_css(page: str) -> str:
    """Inline the highlight palette into the served page's head so the
    single-file dashboard stays self-contained (no sidecar request)."""
    css = _highlight_css()
    if not css:
        return page
    tag = f'<style id="ix-highlight">\n{css}</style>'
    if "</head>" in page:
        return page.replace("</head>", f"{tag}</head>", 1)
    return tag + page


# Read once at startup: the page is an immutable nix-store artifact for the life
# of the server, and the live data arrives over the API rather than the HTML. The
# highlight palette is inlined now so every served copy carries both themes.
_PAGE = _with_highlight_css(_load_page())


async def start(config: Config) -> web.AppRunner:
    app = web.Application()
    conn = store.connect(config.store_path)

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=_PAGE, content_type="text/html")

    def _highlight(rows: list[dict]) -> list[dict]:
        # Highlight each job's source once per unique snippet (cached); the
        # dashboard card renders the spans. This is a dashboard-only view detail,
        # so it is layered here, not in `feed` (an embedder highlights its own way).
        for row in rows:
            row["code_html"] = _code_html(row.get("code") or "")
        return rows

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(_highlight(store.recent(conn, limit=feed.JOBS_LIMIT)))

    async def resources(_request: web.Request) -> web.Response:
        return web.json_response(store.live_resources(conn))

    async def cells(_request: web.Request) -> web.Response:
        return web.json_response(store.cells(conn))

    async def snapshot(_request: web.Request) -> web.Response:
        # The whole presentation in one read, the embed contract an external
        # consumer (the room server) polls; `rev` lets it skip unchanged renders.
        return web.json_response(feed.snapshot(conn))

    async def job(request: web.Request) -> web.Response:
        # One execution by id: the rich outputs for the `jobs['<id>']` a
        # python_exec tool result already names, so an embedder renders that run's
        # tables/plots/HTML beside the tool call.
        one = feed.job(conn, request.match_info["id"])
        if one is None:
            return web.json_response({"error": "no such job"}, status=404)
        return web.json_response(one)

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    app.router.add_get("/api/jobs/{id}", job)
    app.router.add_get("/api/resources", resources)
    app.router.add_get("/api/cells", cells)
    app.router.add_get("/api/snapshot", snapshot)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, config.host, config.dashboard_port)
    await site.start()
    return runner
