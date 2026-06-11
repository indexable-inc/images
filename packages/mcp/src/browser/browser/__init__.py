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
    await browser.get_or_create_browser()        # connect, or launch a visible Chrome
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

To *understand* a page rather than look at it, reach for ``vdom()``: a clean,
filtered virtual DOM -- a compact, machine-readable map of where everything is.
One in-page pass keeps only the nodes that matter (interactive controls,
landmarks, headings, named images), drops hidden / ``aria-hidden`` / script /
style noise, and collapses wrapper ``<div>`` chains, so even a busy page reads as
a small tree. Every node carries its role, accessible name, on-screen box
(x, y, w, h) and a Playwright-ready CSS ``selector``, so the model can decide what
to click without a screenshot. ``read()`` is the lighter text-first sibling (page
prose plus the same elements, flat); ``vdom()`` is the structured map you act on.

    v = await browser.vdom()                      # clean tree of the front tab
    v                                             # render: a compact, indented map
    v.df                                          # the full map as a polars frame
    sel = v.node(12)["selector"]                  # act on a node by its [12] ref
    await (await browser.page()).click(sel)
    await browser.vdom(interactive_only=True)     # leanest map for a complex page
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
    "read",
    "vdom",
    "Vdom",
    "close",
]

__version__ = "0.2.0"

# The standard Chrome DevTools Protocol port. Connect here unless told otherwise,
# so the common case ("a browser is already running with the debug port") needs no
# argument at all.
DEFAULT_ENDPOINT = "http://127.0.0.1:9222"

# The browser to launch when none is running: a stock Chromium-family browser
# whose UI actually renders CDP-created tabs, so the "visible, never headless"
# promise holds -- you can SEE and click the tab `goto` opened. Dia (Arc engine)
# manages its tab strip in its own layer and never shows CDP-created tabs, so a
# page driven in Dia is live but invisible in its UI; pass `app="Dia"` only if
# you know you want that. Any Chromium-family app name (macOS) or executable
# path works.
DEFAULT_APP = "Google Chrome"

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
    like ``"Google Chrome"`` / ``"Dia"``, or an executable path elsewhere);
    ``user_data_dir`` is the profile to launch with (defaults to a dedicated
    ``~/.cdp-<app>-profile`` so it never disturbs your everyday session).

    A LAUNCH (as opposed to a reuse) is announced on stdout: the launched browser
    is a separate instance on a fresh, logged-out profile, possibly behind other
    windows -- without the note, "it worked" is indistinguishable from "nothing
    happened" to the human looking for the window."""
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
    print(
        f"browser: launched a NEW visible {app} instance on {endpoint} with its own "
        f"profile {udd} (a fresh, logged-out session, separate from your everyday "
        f"{app}; its window may be behind others). Future calls reuse it."
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


# Default longest-edge cap for a model-bound shot. A "did the action work?"
# check reads fine at ~1024px, and capping the longest side here (at the source,
# before the bytes ever leave the helper) keeps both the human dashboard copy
# and the model copy small -- the kernel's own llm_images budget only ever sees
# the model copy and runs *after* this. Pass ``max_dim=0`` for no downscale.
_SHOT_MAX_DIM = 1024


def _encode_shot(png: bytes, *, max_dim: int, fmt: str, quality: int) -> tuple[bytes, str]:
    """Downscale a raw screenshot's longest edge to ``max_dim`` (aspect preserved;
    ``max_dim<=0`` disables it) and re-encode as ``fmt`` (``"png"`` or ``"jpeg"``,
    the latter at ``quality``). Returns ``(bytes, mime)``. Pure and never raises:
    if Pillow is missing or anything fails it returns the original PNG untouched,
    so a shot can only ever get smaller, never break. JPEG flattens alpha onto
    white (JPEG has no alpha) so transparency does not render black."""
    try:
        import io

        from PIL import Image

        img = Image.open(io.BytesIO(png))
        img.load()
        width, height = img.size
        longest = max(width, height)
        if max_dim > 0 and longest > max_dim:
            s = max_dim / longest
            img = img.resize((max(1, round(width * s)), max(1, round(height * s))))
        elif fmt == "png":
            # No resize and PNG requested: the original bytes are already a fine,
            # lossless PNG -- avoid a needless re-encode.
            return png, "image/png"
        buf = io.BytesIO()
        if fmt == "jpeg":
            if img.mode != "RGB":
                rgba = img.convert("RGBA")
                flat = Image.new("RGB", rgba.size, (255, 255, 255))
                flat.paste(rgba, mask=rgba.split()[-1])
                img = flat
            img.save(buf, format="JPEG", quality=quality, optimize=True)
            return buf.getvalue(), "image/jpeg"
        if img.mode not in ("RGB", "RGBA", "L"):
            img = img.convert("RGBA")
        img.save(buf, format="PNG", optimize=True)
        return buf.getvalue(), "image/png"
    except Exception:
        return png, "image/png"


async def shot(
    target=None,
    *,
    endpoint: str = DEFAULT_ENDPOINT,
    full_page: bool = False,
    to_model: bool = True,
    scale: str = "css",
    max_dim: int | None = None,
    format: str = "jpeg",
    quality: int = 72,
):
    """Screenshot a ``Page`` (or the front tab when ``target`` is None) and return a
    :class:`Result`: the human sees the image, the model gets the image plus the
    page's title and url. End a cell with it to render the screenshot.

    A screenshot used to confirm "did the UI action work?" should be cheap in
    context, so the defaults are tuned for that, capping cost at the source:

    - ``scale="css"`` captures at CSS-pixel resolution, ignoring the display's
      ``devicePixelRatio`` -- on a 2x (Retina) screen this alone is a 4x pixel
      cut for zero agent benefit. Pass ``scale="device"`` for native-resolution
      pixels when they genuinely matter.
    - ``max_dim`` caps the longest edge (default ``1024``; ``0`` disables). This
      runs *before* the kernel's own ``IX_MCP_IMAGE_MAX_DIM`` model-image budget,
      and unlike that budget it also shrinks the human's dashboard copy.
    - ``format`` / ``quality`` default to JPEG at 72 -- a fraction of a PNG's size
      for a photographic screenshot. Pass ``format="png"`` for a crisp lossless
      shot of UI / diagrams / text.

    Pass ``to_model=False`` for a dashboard-only screenshot: the human sees the
    image, the model gets just the title/url note and zero vision tokens.

    For "did the action work?" verification, prefer the far cheaper :func:`read`
    or :func:`vdom` (page text + interactive elements, no vision tokens) and
    reserve :func:`shot` for when the pixels themselves are what you need to see.
    """
    if max_dim is None:
        max_dim = _SHOT_MAX_DIM
    if format not in ("png", "jpeg"):
        raise ValueError(f"format must be 'png' or 'jpeg', got {format!r}")
    if scale not in ("css", "device"):
        raise ValueError(f"scale must be 'css' or 'device', got {scale!r}")
    pg = target if target is not None else await page(endpoint=endpoint)
    try:
        await pg.bring_to_front()
    except Exception:
        pass
    try:
        png = await pg.screenshot(full_page=full_page, scale=scale)
    except Exception as exc:
        # A just-launched browser window can report a 0-size viewport over CDP;
        # force a sensible viewport and retry once so a screenshot always succeeds.
        if "width" in str(exc).lower() or "height" in str(exc).lower():
            await pg.set_viewport_size({"width": 1280, "height": 800})
            png = await pg.screenshot(full_page=full_page, scale=scale)
        else:
            raise
    data, mime = _encode_shot(png, max_dim=max_dim, fmt=format, quality=quality)
    note = f"{await pg.title()} \u2014 {pg.url}"
    try:
        from ix_notebook_mcp.runtime import Result

        data_uri = f"data:{mime};base64," + _base64.b64encode(data).decode("ascii")
        user_html = f'<img alt="{_html.escape(note)}" src="{data_uri}" style="max-width:100%" />'
        images = [data] if to_model else []
        return Result(user_html=user_html, llm_result=note, llm_images=images)
    except Exception:
        # Outside the kernel (no runtime): hand back the re-encoded image bytes.
        return data


async def read(target=None, *, endpoint: str = DEFAULT_ENDPOINT, max_chars: int = 8000):
    """A cheap, text-first readout of a page (or the front tab when ``target`` is
    None): its title/url, the visible text, and the interactive elements (links,
    buttons, fields) with their role and accessible name -- an accessibility-style
    snapshot for an iterative navigate -> inspect -> click loop WITHOUT the vision
    tokens of a full-res :func:`shot`. Returns a :class:`Result`.

    This is the text-first sibling of :func:`vdom`, built on the very same clean-DOM
    walker: the element list here is exactly ``vdom().interactive``, plus the page's
    visible prose. Reach for :func:`vdom` when you want the structured map to act on
    (geometry + a CSS ``selector`` per node); reach for :func:`shot` only when you
    must SEE layout/visuals. ``max_chars`` caps the visible-text section."""
    pg = target if target is not None else await page(endpoint=endpoint)
    snap = await vdom(pg, interactive_only=True)
    text = await pg.evaluate(
        "() => (document.body ? document.body.innerText : '').replace(/\\s+/g, ' ').trim()"
    )
    clipped = text[:max_chars]
    if len(text) > max_chars:
        clipped += f"\n... [+{len(text) - max_chars} more chars]"
    els = [n for n in snap.flat if n.get("interactive")][:200]
    lines = []
    for e in els:
        name = e.get("name") or ""
        a = e.get("attrs", {})
        extra = a.get("href") or (f"[{a['type']}]" if a.get("type") else "")
        role = e.get("role") or e.get("tag", "?")
        lines.append(f"- {role} {name!r}" + (f" -> {extra}" if extra else ""))
    elements = "\n".join(lines) or "(none)"
    body = (
        f"{snap.title} \u2014 {snap.url}\n\n## visible text\n{clipped}\n\n"
        f"## interactive ({len(els)})\n{elements}"
    )
    try:
        from ix_notebook_mcp.runtime import Result

        head = _html.escape(f"{snap.title} \u2014 {snap.url}")
        user_html = (
            f'<div style="font:13px ui-monospace,monospace"><b>{head}</b>'
            f'<pre style="white-space:pre-wrap;max-height:24em;overflow:auto">{_html.escape(body)}</pre></div>'
        )
        return Result(user_html=user_html, llm_result=body)
    except Exception:
        return body


# ---------------------------------------------------------------------------
# Clean virtual DOM: a filtered, machine-readable map of the page.
# ---------------------------------------------------------------------------
#
# A screenshot tells the model what a page LOOKS like; `vdom()` tells it what the
# page IS and WHERE everything is -- as compact, machine-readable structure rather
# than pixels. One pass in the page walks the live DOM and returns only the nodes
# that matter (interactive controls, landmarks, headings, named images), pruning
# scripts/styles/comments/hidden/`aria-hidden` nodes and collapsing the wrapper
# `<div>` chains that make raw HTML unreadable. Each kept node carries its role, a
# concise accessible name, the attributes worth acting on (href/type/value/...),
# its on-screen box (x, y, w, h) and a CSS `selector` you can hand straight to
# Playwright. So the agent can decide what to click WITHOUT a screenshot.

# Injected into the page by `vdom()`; returns the clean tree as plain JSON.
_VDOM_JS = r"""
(opts) => {
  const maxText = (opts && opts.maxText) || 120;
  const viewportOnly = !!(opts && opts.viewportOnly);
  const interactiveOnly = !!(opts && opts.interactiveOnly);

  const IMPLICIT_ROLE = {
    a: 'link', button: 'button', input: 'textbox', select: 'combobox',
    textarea: 'textbox', img: 'image', nav: 'navigation', main: 'main',
    header: 'banner', footer: 'contentinfo', aside: 'complementary',
    form: 'form', section: 'region', article: 'article', dialog: 'dialog',
    h1: 'heading', h2: 'heading', h3: 'heading', h4: 'heading', h5: 'heading',
    h6: 'heading', ul: 'list', ol: 'list', li: 'listitem', table: 'table',
    summary: 'button', option: 'option', label: 'label',
  };
  const INTERACTIVE_TAGS = new Set(
    ['a','button','input','select','textarea','summary','details','label','option','video','audio','iframe']);
  const LANDMARK_TAGS = new Set(
    ['main','nav','header','footer','aside','form','section','article','dialog']);
  const HEADING_TAGS = new Set(['h1','h2','h3','h4','h5','h6']);
  const SKIP_TAGS = new Set(
    ['script','style','noscript','template','head','meta','link','br','path','svg','defs','g']);

  const clamp = (s) => {
    s = (s || '').replace(/\s+/g, ' ').trim();
    return s.length > maxText ? s.slice(0, maxText - 1) + '…' : s;
  };

  const cssEscape = (s) =>
    (window.CSS && CSS.escape) ? CSS.escape(s) : String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&');

  // A short, reasonably-unique CSS selector for `el`.
  const selectorFor = (el) => {
    if (el.id && document.querySelectorAll('#' + cssEscape(el.id)).length === 1)
      return '#' + cssEscape(el.id);
    const parts = [];
    let cur = el;
    while (cur && cur.nodeType === 1 && cur.tagName.toLowerCase() !== 'html') {
      let part = cur.tagName.toLowerCase();
      if (cur.id) { parts.unshift('#' + cssEscape(cur.id)); break; }
      const parent = cur.parentElement;
      if (parent) {
        const sibs = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
        if (sibs.length > 1) part += ':nth-of-type(' + (sibs.indexOf(cur) + 1) + ')';
      }
      parts.unshift(part);
      cur = cur.parentElement;
    }
    return parts.join(' > ');
  };

  const directText = (el) => {
    let t = '';
    for (const n of el.childNodes)
      if (n.nodeType === 3) t += n.nodeValue;
    return t.replace(/\s+/g, ' ').trim();
  };

  const accName = (el) => {
    const tag = el.tagName.toLowerCase();
    const aria = el.getAttribute('aria-label');
    if (aria) return clamp(aria);
    const labelledby = el.getAttribute('aria-labelledby');
    if (labelledby) {
      const parts = labelledby.split(/\s+/)
        .map(id => { const r = document.getElementById(id); return r ? r.innerText : ''; })
        .filter(Boolean);
      if (parts.length) return clamp(parts.join(' '));
    }
    if (tag === 'img') return clamp(el.getAttribute('alt') || '');
    if (tag === 'input') {
      const type = (el.getAttribute('type') || 'text').toLowerCase();
      if (type === 'submit' || type === 'button') return clamp(el.value);
      return clamp(el.getAttribute('placeholder') || el.getAttribute('aria-label') || el.value || '');
    }
    if (tag === 'select' || tag === 'textarea')
      return clamp(el.getAttribute('placeholder') || el.getAttribute('aria-label') || '');
    const own = directText(el);
    if (own) return clamp(own);
    return clamp(el.innerText || el.getAttribute('title') || '');
  };

  const visible = (el, rect) => {
    if (rect.width <= 1 || rect.height <= 1) return false;
    const st = getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none' || +st.opacity === 0) return false;
    if (viewportOnly) {
      const vw = innerWidth, vh = innerHeight;
      if (rect.bottom < 0 || rect.right < 0 || rect.top > vh || rect.left > vw) return false;
    }
    return true;
  };

  const KEEP_ATTRS = ['href','type','placeholder','value','name','aria-label','title','alt','for','role','checked','selected','disabled','aria-expanded','aria-checked'];

  // Collapse a run of consecutive short text-only leaves (syntax-highlight token
  // soup, operator spans, breadcrumb bits) into ONE text node, so a highlighted
  // code block or a chip row reads as a single line instead of dozens. Long text
  // (paragraphs) and anything interactive/structural is left untouched.
  const isLeafText = (n) =>
    n && !n.interactive && !n.group && (!n.children || n.children.length === 0) &&
    n.name && n.tag !== 'img' && n.tag !== 'iframe' && n.role !== 'heading' &&
    n.name.length <= 30;
  const mergeRuns = (arr) => {
    const out = [];
    let run = [];
    const flush = () => {
      if (run.length === 0) return;
      if (run.length === 1) { out.push(run[0]); run = []; return; }
      const x = Math.min(...run.map(r => r.x));
      const y = Math.min(...run.map(r => r.y));
      const w = Math.max(...run.map(r => r.x + r.w)) - x;
      const h = Math.max(...run.map(r => r.y + r.h)) - y;
      out.push({ tag: 'text', role: 'text', name: clamp(run.map(r => r.name).join(' ')),
                 interactive: false, x, y, w, h, selector: '', attrs: {}, group: false, children: [] });
      run = [];
    };
    for (const n of arr) { if (isLeafText(n)) run.push(n); else { flush(); out.push(n); } }
    flush();
    return out;
  };

  const build = (el, inLabel) => {
    const tag = el.tagName ? el.tagName.toLowerCase() : '';
    if (!tag || SKIP_TAGS.has(tag)) return [];
    if (el.getAttribute && el.getAttribute('aria-hidden') === 'true') return [];

    const rect = el.getBoundingClientRect();
    const isVisible = visible(el, rect);

    const role0 = (el.getAttribute && el.getAttribute('role')) || IMPLICIT_ROLE[tag] || '';
    const interactive0 =
      INTERACTIVE_TAGS.has(tag) ||
      (el.getAttribute && (el.getAttribute('onclick') !== null ||
        el.getAttribute('contenteditable') === 'true' ||
        (el.getAttribute('tabindex') !== null && +el.getAttribute('tabindex') >= 0) ||
        ['button','link','checkbox','radio','tab','menuitem','switch','option','textbox','combobox','searchbox','slider']
          .includes(el.getAttribute('role'))));
    const heading0 = HEADING_TAGS.has(tag);
    const childInLabel = inLabel || interactive0 || heading0;

    // Recurse first so we can collapse pure wrappers.
    let kids = [];
    for (const c of el.children) kids.push(...build(c, childInLabel));
    kids = mergeRuns(kids);

    const role = role0, interactive = interactive0, heading = heading0;
    const landmark = LANDMARK_TAGS.has(tag);
    const name = accName(el);
    const interesting =
      isVisible && (interactive || landmark || heading || tag === 'iframe' ||
        (tag === 'img' && (name || !interactiveOnly)) ||
        (!interactiveOnly && name && directText(el) && !inLabel));

    if (!interesting) {
      if (kids.length === 0) return [];
      if (kids.length === 1) return kids;        // collapse transparent wrapper
      // a branching container: keep a lightweight group so structure survives
      return [{ tag, role: role || 'group', name: '', interactive: false,
                x: Math.round(rect.x), y: Math.round(rect.y),
                w: Math.round(rect.width), h: Math.round(rect.height),
                selector: '', attrs: {}, group: true, children: kids }];
    }

    const attrs = {};
    if (el.getAttributeNames) for (const a of el.getAttributeNames())
      if (KEEP_ATTRS.includes(a)) {
        const v = el.getAttribute(a);
        if (v !== null && v !== '') attrs[a] = clamp(v);
      }

    return [{
      tag, role, name, interactive,
      x: Math.round(rect.x), y: Math.round(rect.y),
      w: Math.round(rect.width), h: Math.round(rect.height),
      selector: selectorFor(el), attrs, group: false, children: kids,
    }];
  };

  const roots = mergeRuns(build(document.body, false));
  return {
    url: location.href,
    title: document.title,
    viewport: { w: innerWidth, h: innerHeight, scrollX: Math.round(scrollX), scrollY: Math.round(scrollY) },
    nodes: roots,
  };
}
"""


def _walk_assign(nodes, _counter=None, _depth=0, _out=None):
    """Number the kept (non-group) nodes in document order and flatten the tree.

    Refs are 1-based and stable within one snapshot, so `[12]` in the rendered
    tree, the `ref` column of `.df`, and `vdom.node(12)` all denote the same node.
    """
    if _counter is None:
        _counter, _out = [0], []
    for n in nodes:
        if n.get("group"):
            n["ref"] = None
        else:
            _counter[0] += 1
            n["ref"] = _counter[0]
        n["depth"] = _depth
        _out.append(n)
        _walk_assign(n.get("children", ()), _counter, _depth + 1, _out)
    return _out


_PRUNE = ("children", "group", "depth")


def _node_public(n: dict) -> dict:
    """A node as the model/JSON sees it: drop tree-walk bookkeeping, keep the map."""
    return {k: v for k, v in n.items() if k not in _PRUNE}


class Vdom:
    """A clean, filtered snapshot of a page's DOM: the recommended way to understand
    a page's structure and find what to act on.

    Render it (end a cell with it) for a compact, indented tree -- the model reads
    that text, the human sees the same tree as styled HTML. The full machine-readable
    map is always one attribute away and never truncated:

        v = await browser.vdom()
        v                       # compact tree (capped; the glance)
        v.df                    # every node as a polars frame: ref, role, name, x, y, w, h, selector
        v.json                  # the nested tree as plain JSON (dicts/lists)
        v.interactive           # just the actionable nodes (a frame)
        v.node(12)              # the dict for ref 12 (its selector, box, attrs)
        await browser.goto(...); (await browser.vdom(interactive_only=True))  # leanest map

    For big, complex pages prefer ``interactive_only=True`` (drop body text, keep the
    controls) and/or ``viewport_only=True`` (only what is on screen) so the snapshot
    stays small instead of dumping thousands of nodes.
    """

    # How many tree lines the text/HTML glance shows before pointing at `.df` for
    # the rest. The full map is always in `.df` / `.json`; this only caps the glance.
    RENDER_LIMIT = 200

    def __init__(self, raw: dict):
        self.url: str = raw.get("url", "")
        self.title: str = raw.get("title", "")
        self.viewport: dict = raw.get("viewport", {})
        self.nodes: list = raw.get("nodes", [])
        self.flat: list = _walk_assign(self.nodes)

    # -- machine-readable views ------------------------------------------------
    @property
    def json(self) -> list:
        """The clean DOM as a nested list of plain dicts (JSON-serializable)."""

        def strip(nodes):
            out = []
            for n in nodes:
                d = _node_public(n)
                d["children"] = strip(n.get("children", ()))
                out.append(d)
            return out

        return strip(self.nodes)

    @property
    def df(self):
        """Every kept node as a polars frame -- the full, untruncated page map:
        one row per node with its ref, role, name, geometry and CSS selector."""
        import polars as pl

        rows = []
        for n in self.flat:
            a = n.get("attrs", {})
            rows.append(
                {
                    "ref": n.get("ref"),
                    "depth": n["depth"],
                    "role": n.get("role") or n.get("tag", ""),
                    "tag": n.get("tag", ""),
                    "name": n.get("name", ""),
                    "interactive": bool(n.get("interactive")),
                    "href": a.get("href", ""),
                    "type": a.get("type", ""),
                    "x": n.get("x"),
                    "y": n.get("y"),
                    "w": n.get("w"),
                    "h": n.get("h"),
                    "selector": n.get("selector", ""),
                }
            )
        schema = ["ref", "depth", "role", "tag", "name", "interactive",
                  "href", "type", "x", "y", "w", "h", "selector"]
        return pl.DataFrame(rows, schema=schema) if rows else pl.DataFrame(schema=schema)

    @property
    def interactive(self):
        """Just the actionable nodes (links, buttons, fields, ...) as a polars frame."""
        return self.df.filter(__import__("polars").col("interactive"))

    def node(self, ref: int) -> dict | None:
        """The public dict for a node by its ``ref`` (selector, box, attrs)."""
        for n in self.flat:
            if n.get("ref") == ref:
                return _node_public(n)
        return None

    # -- glance (capped) -------------------------------------------------------
    def _lines(self) -> list[str]:
        lines: list[str] = []

        def emit(nodes, depth):
            for n in nodes:
                pad = "  " * depth
                if n.get("group"):
                    lines.append(f"{pad}<{n.get('tag', 'div')}>")
                else:
                    a = n.get("attrs", {})
                    role = n.get("role") or n.get("tag", "")
                    mark = "*" if n.get("interactive") else " "
                    nm = f' "{n["name"]}"' if n.get("name") else ""
                    extra = ""
                    if a.get("href"):
                        extra += f" href={a['href']}"
                    if a.get("type"):
                        extra += f" type={a['type']}"
                    box = f"  @{n.get('x')},{n.get('y')} {n.get('w')}x{n.get('h')}"
                    lines.append(f"{pad}[{n['ref']}] {mark}{role}{nm}{extra}{box}")
                emit(n.get("children", ()), depth + 1)

        emit(self.nodes, 0)
        return lines

    def text(self, limit: int | None = None) -> str:
        """The indented tree as plain text, capped at ``limit`` lines (the glance)."""
        limit = self.RENDER_LIMIT if limit is None else limit
        n_real = sum(1 for n in self.flat if not n.get("group"))
        head = (
            f"{self.title or '(untitled)'} \u2014 {self.url}\n"
            f"{n_real} nodes "
            f"({sum(1 for n in self.flat if n.get('interactive'))} interactive) "
            f"\u00b7 viewport {self.viewport.get('w')}x{self.viewport.get('h')}"
        )
        lines = self._lines()
        body = lines if limit <= 0 or len(lines) <= limit else lines[:limit] + [
            f"\u2026 +{len(lines) - limit} more lines \u2014 see .df for the full map, "
            f"or re-run with interactive_only=True / viewport_only=True"
        ]
        return head + "\n" + "\n".join(body)

    def __repr__(self) -> str:
        return self.text()

    def _repr_html_(self) -> str:
        rows = []
        for ln in self.text().split("\n")[1:]:  # skip the header line; shown separately
            rows.append(_html.escape(ln))
        title = _html.escape(self.title or "(untitled)")
        n_real = sum(1 for n in self.flat if not n.get("group"))
        n_int = sum(1 for n in self.flat if n.get("interactive"))
        head = (
            f'<div style="font:600 13px system-ui;margin-bottom:4px">{title}'
            f'<span style="font-weight:400;opacity:.6"> \u2014 {_html.escape(self.url)}</span></div>'
            f'<div style="font:400 11px system-ui;opacity:.6;margin-bottom:6px">'
            f"{n_real} nodes \u00b7 {n_int} interactive \u00b7 "
            f'viewport {self.viewport.get("w")}\u00d7{self.viewport.get("h")}</div>'
        )
        pre = (
            '<pre style="font:12px ui-monospace,monospace;line-height:1.45;margin:0;'
            'white-space:pre;overflow-x:auto">' + "\n".join(rows) + "</pre>"
        )
        return head + pre


async def vdom(
    target=None,
    *,
    endpoint: str = DEFAULT_ENDPOINT,
    interactive_only: bool = False,
    viewport_only: bool = False,
    max_text: int = 120,
):
    """A clean, filtered :class:`Vdom` of a page -- the recommended way to read a
    page's structure and decide what to act on, without a screenshot.

    One in-page pass keeps only meaningful nodes (interactive controls, landmarks,
    headings, named images), drops hidden / ``aria-hidden`` / script / style nodes,
    and collapses wrapper ``<div>`` chains. Each node carries its role, accessible
    name, key attributes, on-screen box and a Playwright-ready CSS ``selector``.

    ``target`` is a Playwright ``Page`` (defaults to the front tab). For big pages
    pass ``interactive_only=True`` to keep just the controls, and/or
    ``viewport_only=True`` to keep only what is currently on screen, so the snapshot
    stays small. ``max_text`` bounds each accessible name's length.
    """
    opts = {
        "interactiveOnly": interactive_only,
        "viewportOnly": viewport_only,
        "maxText": max_text,
    }
    pg = target if target is not None else await page(endpoint=endpoint)
    try:
        raw = await pg.evaluate(_VDOM_JS, opts)
    except Exception as exc:
        # The front tab can be torn down underneath the evaluate (the user closed
        # it, the browser swapped contexts). When WE picked the page, re-resolve
        # the front tab once and retry; a caller-supplied page is theirs to own,
        # so its closure propagates.
        if target is not None or not _target_closed(exc):
            raise
        pg = await page(endpoint=endpoint)
        raw = await pg.evaluate(_VDOM_JS, opts)
    return Vdom(raw)


def _target_closed(exc: Exception) -> bool:
    """True when ``exc`` is Playwright's page/context/browser-closed error."""
    try:
        from playwright._impl._errors import TargetClosedError
    except Exception:
        return "has been closed" in str(exc)
    return isinstance(exc, TargetClosedError) or "has been closed" in str(exc)


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
