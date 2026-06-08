"""Drive a running browser over the Chrome DevTools Protocol (CDP) with Playwright.

Bundled like ``view``/``fff``/``sh``, and Playwright itself is bundled in the
interpreter, so a session can ``import browser`` and drive a real browser with no
``pip``/``uv install`` and no ``playwright install`` step.

The model: a Chromium-family browser (Dia, Chrome, Edge, Brave) listens on a CDP
``--remote-debugging-port`` -- the standard 9222 by default -- and ``browser``
connects to it on the kernel's own event loop (so it never blocks other jobs) and
hands you a live Playwright ``Page``. The Playwright driver and the CDP connection
are started once and cached, so successive calls reuse the same session.

    import browser
    await browser.get_or_create_browser()        # connect, or launch a visible Dia
    await browser.goto("https://example.com")     # navigate the front tab
    await browser.shot()                          # screenshot -> a Result (image)
    await browser.read()                          # cheap text+elements readout (no vision tokens)
    page = await browser.page()                   # the live Page: full Playwright API

    # Drive a page (every Playwright call is awaited; reuse the one `page`):
    await page.fill("input[name=q]", "ix mcp")    # type into a field
    await page.click("text=Sign in")              # click by text / CSS / role
    await page.wait_for_selector(".results")      # wait for an element...
    await page.wait_for_load_state("networkidle")  # ...or for the page to settle
    text = await page.inner_text(".results")      # read text (or .text_content / .content)
    await page.screenshot()                       # bare bytes auto-render as an image

`await page.screenshot()` returns raw PNG bytes; ending a cell with it (or with
`browser.shot()`) renders the image inline for the human and the model -- so the
"do something, then look" loop is one obvious flow. Never `print()` a screenshot:
its byte repr is a ~50k-char wall that blows the result cap.

``get_or_create_browser()`` is the entry point: it connects to whatever is already
on the port, and otherwise launches a real, on-screen browser listening there. It
is **never headless** -- a launched browser is a visible window a human can watch
and take over. If a browser is already on the port, its profile is used as-is;
when launching, a dedicated CDP profile (``~/.cdp-<app>-profile``) is used so it
runs as its own instance without disturbing your everyday browser session.

``shot()`` returns a :class:`Result`, so ending a cell with it shows the screenshot
to the human and hands the model the same image. Every other call returns the raw
Playwright object (``Browser`` / ``BrowserContext`` / ``Page``), so the full async
Playwright API is yours.
"""

from __future__ import annotations

import asyncio as _asyncio
import base64 as _base64
import html as _html
import os as _os
import sys as _sys
import urllib.parse as _urlparse

__all__ = [
    "DEFAULT_ENDPOINT",
    "DEFAULT_APP",
    "get_or_create_browser",
    "connect",
    "context",
    "page",
    "goto",
    "shot",
    "close",
]

__version__ = "0.1.0"

# The standard Chrome DevTools Protocol port. Connect here unless told otherwise,
# so the common case ("a browser is already running with the debug port") needs no
# argument at all.
DEFAULT_ENDPOINT = "http://127.0.0.1:9222"

# The browser to launch when none is running. Dia is the default on this fleet; a
# caller can pass any Chromium-family app name (macOS) or executable path.
DEFAULT_APP = "Dia"

# Started once per kernel and reused: the Playwright driver process and one CDP
# connection per endpoint. Module-level so the live browser session survives across
# `python_exec` cells instead of being torn down when a function returns.
_playwright = None
_browsers: dict = {}


async def _ensure_playwright():
    global _playwright
    if _playwright is None:
        from playwright.async_api import async_playwright

        _playwright = await async_playwright().start()
    return _playwright


def _port_of(endpoint: str) -> int:
    """The TCP port in a CDP endpoint URL (``http://host:port``)."""
    port = _urlparse.urlsplit(endpoint).port
    if port is None:
        raise ValueError(f"endpoint has no port: {endpoint!r}")
    return port


def _default_user_data_dir(app: str) -> str:
    """A dedicated, module-owned profile dir for a launched browser, so it runs as
    its own instance without touching the user's everyday session."""
    slug = "".join(c if c.isalnum() else "-" for c in app.lower()).strip("-")
    return _os.path.expanduser(f"~/.cdp-{slug}-profile")


def _launch_argv(app: str, port: int, user_data_dir: str) -> list[str]:
    """The argv to launch a *visible* Chromium-family browser listening on ``port``.

    On macOS this is ``open -na <app> --args ...`` (``app`` is an application name);
    elsewhere ``app`` is run directly as an executable. It is deliberately NEVER
    headless: the launched browser is an on-screen window."""
    flags = [
        f"--remote-debugging-port={port}",
        f"--user-data-dir={user_data_dir}",
        "--no-first-run",
        "--no-default-browser-check",
    ]
    if _sys.platform == "darwin":
        return ["open", "-na", app, "--args", *flags]
    return [app, *flags]


async def _cdp_ready(endpoint: str, *, timeout: float) -> bool:
    """Poll the CDP version endpoint until it answers or ``timeout`` (seconds)."""
    import httpx

    deadline = _asyncio.get_running_loop().time() + timeout
    async with httpx.AsyncClient() as client:
        while _asyncio.get_running_loop().time() < deadline:
            try:
                resp = await client.get(f"{endpoint}/json/version", timeout=1.0)
                if resp.status_code == 200:
                    return True
            except Exception:
                pass
            await _asyncio.sleep(0.25)
    return False


async def get_or_create_browser(
    *,
    endpoint: str = DEFAULT_ENDPOINT,
    app: str = DEFAULT_APP,
    user_data_dir: str | None = None,
    timeout: float = 30.0,
):
    """Connect to a browser already listening on ``endpoint``, or launch a real,
    **visible** (never headless) browser there and connect to it. Returns the live
    Playwright ``Browser``.

    ``app`` is the browser to launch when none is running (a macOS application name
    like ``"Dia"`` / ``"Google Chrome"``, or an executable path elsewhere);
    ``user_data_dir`` is the profile to launch with (defaults to a dedicated
    ``~/.cdp-<app>-profile`` so it never disturbs your everyday session)."""
    try:
        return await connect(endpoint)
    except Exception:
        pass
    port = _port_of(endpoint)
    udd = user_data_dir or _default_user_data_dir(app)
    _os.makedirs(udd, exist_ok=True)
    argv = _launch_argv(app, port, udd)
    # Detach into its own session: `open` returns at once, and a directly-exec'd
    # browser must outlive this call. Output is discarded so it never holds a pipe.
    await _asyncio.create_subprocess_exec(
        *argv,
        stdout=_asyncio.subprocess.DEVNULL,
        stderr=_asyncio.subprocess.DEVNULL,
        start_new_session=True,
    )
    if not await _cdp_ready(endpoint, timeout=timeout):
        raise TimeoutError(
            f"launched {app!r} but no CDP endpoint came up at {endpoint} within {timeout}s "
            f"(argv: {argv})"
        )
    return await connect(endpoint)


async def connect(endpoint: str = DEFAULT_ENDPOINT):
    """Connect to a running browser's CDP endpoint (or reuse the cached connection)
    and return the Playwright ``Browser``. ``endpoint`` is the DevTools HTTP URL
    ``http://host:port`` -- the default is the standard CDP port 9222.

    Raises ``ConnectionError`` if nothing is listening; use
    :func:`get_or_create_browser` to launch one automatically instead."""
    existing = _browsers.get(endpoint)
    if existing is not None and existing.is_connected():
        return existing
    pw = await _ensure_playwright()
    try:
        browser = await pw.chromium.connect_over_cdp(endpoint)
    except Exception as exc:
        raise ConnectionError(
            f"no browser reachable at {endpoint} over CDP ({exc}). Use "
            f"await browser.get_or_create_browser() to launch a visible one automatically."
        ) from exc
    _browsers[endpoint] = browser
    return browser


async def context(endpoint: str = DEFAULT_ENDPOINT):
    """The browser's first existing context (its running profile), creating one only
    if the browser exposes none."""
    b = await connect(endpoint)
    return b.contexts[0] if b.contexts else await b.new_context()


async def page(*, endpoint: str = DEFAULT_ENDPOINT, new: bool = False):
    """A Playwright ``Page`` on the running browser: the front (most recent) tab by
    default, or a fresh tab with ``new=True``. Hand it to the full Playwright API."""
    ctx = await context(endpoint)
    if new or not ctx.pages:
        return await ctx.new_page()
    return ctx.pages[-1]


async def goto(
    url: str,
    *,
    endpoint: str = DEFAULT_ENDPOINT,
    new: bool = False,
    wait_until: str = "load",
    timeout: float = 30000,
):
    """Navigate a tab to ``url`` and return its ``Page``. Reuses the front tab unless
    ``new=True``. ``wait_until`` is Playwright's load state (``load`` /
    ``domcontentloaded`` / ``networkidle``); ``timeout`` is in milliseconds."""
    pg = await page(endpoint=endpoint, new=new)
    await pg.goto(url, wait_until=wait_until, timeout=timeout)
    return pg


async def shot(target=None, *, endpoint: str = DEFAULT_ENDPOINT, full_page: bool = False):
    """Screenshot a ``Page`` (or the front tab when ``target`` is None) and return a
    :class:`Result`: the human sees the image, the model gets the PNG plus the page's
    title and url. End a cell with it to render the screenshot."""
    pg = target if target is not None else await page(endpoint=endpoint)
    try:
        await pg.bring_to_front()
    except Exception:
        pass
    try:
        png = await pg.screenshot(full_page=full_page)
    except Exception as exc:
        # A just-launched browser window can report a 0-size viewport over CDP;
        # force a sensible viewport and retry once so a screenshot always succeeds.
        if "width" in str(exc).lower() or "height" in str(exc).lower():
            await pg.set_viewport_size({"width": 1280, "height": 800})
            png = await pg.screenshot(full_page=full_page)
        else:
            raise
    note = f"{await pg.title()} \u2014 {pg.url}"
    try:
        from ix_notebook_mcp.runtime import Result

        data_uri = "data:image/png;base64," + _base64.b64encode(png).decode("ascii")
        user_html = f'<img alt="{_html.escape(note)}" src="{data_uri}" style="max-width:100%" />'
        return Result(user_html=user_html, llm_result=note, llm_images=[png])
    except Exception:
        # Outside the kernel (no runtime): hand back the raw PNG bytes.
        return png


async def read(target=None, *, endpoint: str = DEFAULT_ENDPOINT, max_chars: int = 8000):
    """A cheap, text-first readout of a page (or the front tab when ``target`` is
    None): its title/url, visible text, and the interactive elements (links,
    buttons, inputs) with their role and accessible name -- an accessibility-style
    snapshot for an iterative navigate -> inspect -> click loop WITHOUT the vision
    tokens of a full-res :func:`shot`. Returns a :class:`Result`. Reach for `shot()`
    only when you actually need to SEE layout/visuals; `read()` is enough to find
    and click things. ``max_chars`` caps the visible-text section."""
    pg = target if target is not None else await page(endpoint=endpoint)
    snapshot = await pg.evaluate(
        """() => {
          const vis = el => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
            return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
          const norm = s => (s || '').replace(/\\s+/g, ' ').trim();
          const out = [];
          const sel = 'a[href], button, input, select, textarea, [role=button], [role=link], [role=checkbox], [role=tab], [contenteditable=true]';
          for (const el of document.querySelectorAll(sel)) {
            if (!vis(el)) continue;
            const tag = el.tagName.toLowerCase();
            out.push({
              role: el.getAttribute('role') || tag,
              name: norm(el.getAttribute('aria-label') || el.innerText || el.value || el.getAttribute('placeholder') || el.getAttribute('name') || el.getAttribute('title')).slice(0, 100),
              href: el.getAttribute('href') || '',
              type: el.getAttribute('type') || '',
            });
            if (out.length >= 200) break;
          }
          return { text: norm(document.body ? document.body.innerText : ''), els: out };
        }"""
    )
    title = await pg.title()
    text = snapshot.get("text", "")
    clipped = text[:max_chars]
    if len(text) > max_chars:
        clipped += f"\n... [+{len(text) - max_chars} more chars]"
    lines = []
    for e in snapshot.get("els", []):
        name = e.get("name") or ""
        extra = e.get("href") or (f"[{e['type']}]" if e.get("type") else "")
        lines.append(f"- {e.get('role', '?')} {name!r}" + (f" -> {extra}" if extra else ""))
    elements = "\n".join(lines) or "(none)"
    body = (
        f"{title} \u2014 {pg.url}\n\n## visible text\n{clipped}\n\n"
        f"## interactive ({len(snapshot.get('els', []))})\n{elements}"
    )
    try:
        from ix_notebook_mcp.runtime import Result

        head = _html.escape(f"{title} \u2014 {pg.url}")
        user_html = (
            f'<div style="font:13px ui-monospace,monospace"><b>{head}</b>'
            f'<pre style="white-space:pre-wrap;max-height:24em;overflow:auto">{_html.escape(body)}</pre></div>'
        )
        return Result(user_html=user_html, llm_result=body)
    except Exception:
        return body


async def close(endpoint: str | None = None):
    """Disconnect a cached CDP connection (or all of them) and stop the Playwright
    driver once none remain. This closes the *connection*, not the browser itself."""
    global _playwright
    targets = list(_browsers) if endpoint is None else [endpoint]
    for ep in targets:
        b = _browsers.pop(ep, None)
        if b is not None:
            try:
                await b.close()
            except Exception:
                pass
    if not _browsers and _playwright is not None:
        try:
            await _playwright.stop()
        finally:
            _playwright = None
