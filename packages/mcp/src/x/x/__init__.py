"""Read recent X (Twitter) posts into polars, by driving your logged-in browser.

Bundled into the ix-mcp interpreter like ``browser`` and ``view``, so a session
can ``import x`` with no install step. X has no usable unauthenticated read API
(the public syndication endpoints now return empty timelines), so this module
reads what you can actually see: it drives the browser you are already signed in
to over the Chrome DevTools Protocol -- the same one :mod:`browser` connects to --
navigates to a timeline, profile, search, or thread, scrolls until it has enough
posts, and parses the rendered tweets into a ``polars`` DataFrame.

    import x

    await x.posts()                # your home timeline, most recent first
    await x.posts("@andrewgazelka")  # a user's recent posts
    await x.posts("notifications")   # your notifications
    await x.posts("#rustlang")       # a search
    await x.posts("rust async runtime")  # any text is a search
    await x.posts("https://x.com/Plambey/status/2064307527114793062")  # a thread

Each call returns one row per post with: ``id``, ``handle``, ``author``,
``time`` (UTC datetime), ``text``, ``replies`` / ``reposts`` / ``likes`` /
``views`` / ``bookmarks`` (counts, when shown), ``url`` (the permalink), and
``is_reply``. Pass ``limit`` for how many posts to collect (the page is scrolled
to load more) and ``scroll=False`` to read only what is on screen.

This reuses :mod:`browser`, so the first call connects to a browser already
listening on the CDP port (``http://127.0.0.1:9222`` by default) or launches a
visible one (Dia by default). Reading a timeline that requires sign-in needs that
browser to be signed in to X; a thread or public profile usually reads even
signed out. Cross-platform.
"""

from __future__ import annotations

import asyncio
import os

import polars as pl

import browser

__all__ = ["posts", "DEFAULT_ENDPOINT", "DEFAULT_APP"]

__version__ = "0.1.0"

# Match the browser module's defaults so `x` and `browser` drive the same session.
DEFAULT_ENDPOINT = browser.DEFAULT_ENDPOINT
DEFAULT_APP = browser.DEFAULT_APP

# A shared (multiplayer) room marks the MCP it replicates across participants with
# this env var. Reading X timelines/notifications/bookmarks exposes the signed-in
# account's personal data, so -- like `google_auth` -- it is confined to incognito
# sessions (the default for a plain ix-mcp); only a truthy value refuses access.
SHARED_ENV = "IX_MCP_SHARED"


def _require_incognito() -> None:
    """Refuse to read personal X data in a shared (multiplayer) room.

    `x.posts()` reads whatever the signed-in browser can see (home timeline,
    notifications, bookmarks, a private account's posts), so a shared room would
    leak one person's feed into state everyone can see. A shared room sets
    ``IX_MCP_SHARED``; only then is access refused.
    """
    if os.environ.get(SHARED_ENV):
        raise RuntimeError(
            "x.posts is not available in a shared (multiplayer) room "
            "(IX_MCP_SHARED is set), because it would expose the signed-in "
            "X account's personal feed to everyone in the room. Use it from an "
            "incognito chat instead; its transcript stays private to you."
        )

# Named timeline shortcuts: a bare keyword maps to its x.com path. Anything else
# is treated as a handle (leading "@"), a search ("#tag" or free text), or a URL.
_NAMED = {
    "home": "https://x.com/home",
    "notifications": "https://x.com/notifications",
    "bookmarks": "https://x.com/i/bookmarks",
    "explore": "https://x.com/explore",
    "following": "https://x.com/home",
}

# The post schema, fixed so an empty result still has the right columns/dtypes.
_SCHEMA: dict[str, pl.DataType] = {
    "id": pl.Utf8,
    "handle": pl.Utf8,
    "author": pl.Utf8,
    "time": pl.Datetime("us", "UTC"),
    "text": pl.Utf8,
    "replies": pl.Int64,
    "reposts": pl.Int64,
    "likes": pl.Int64,
    "views": pl.Int64,
    "bookmarks": pl.Int64,
    "url": pl.Utf8,
    "is_reply": pl.Boolean,
}

# In-page extraction: one pass over the rendered <article> tweets. Engagement
# numbers are read from the action bar's aria-label (e.g. "75 replies, 3 reposts,
# ...") because the visible chips are abbreviated ("1.2K") while the label carries
# the exact count. Returns plain JSON so the result crosses the CDP bridge cleanly.
_EXTRACT_JS = r"""
() => {
  const norm = s => (s || '').replace(/ /g, ' ').trim();
  const arts = Array.from(
    document.querySelectorAll('article[data-testid="tweet"], article[role="article"]')
  );
  return arts.map(a => {
    const textEl = a.querySelector('[data-testid="tweetText"]');
    const text = textEl ? norm(textEl.innerText) : '';

    let url = null, id = null, time = null;
    const timeEl = a.querySelector('time');
    const timeA = timeEl ? timeEl.closest('a') : null;
    if (timeA) {
      url = timeA.href;
      time = timeEl.getAttribute('datetime');
      const m = url.match(/status\/(\d+)/);
      if (m) id = m[1];
    }

    let author = null, handle = null;
    const nameEl = a.querySelector('[data-testid="User-Name"]');
    if (nameEl) {
      const parts = norm(nameEl.innerText).split('\n').map(s => s.trim()).filter(Boolean);
      author = parts[0] || null;
      const h = parts.find(p => p.startsWith('@'));
      handle = h ? h.slice(1) : null;
    }
    if (!handle && url) {
      const m = url.match(/(?:x|twitter)\.com\/([^/]+)\/status/);
      if (m) handle = m[1];
    }

    let replies = null, reposts = null, likes = null, views = null, bookmarks = null;
    const group = a.querySelector('[role="group"][aria-label]');
    if (group) {
      const al = group.getAttribute('aria-label').toLowerCase();
      const grab = re => { const m = al.match(re); return m ? parseInt(m[1].replace(/,/g, '')) : null; };
      replies = grab(/([\d,]+)\s+repl/);
      reposts = grab(/([\d,]+)\s+repost/);
      likes = grab(/([\d,]+)\s+like/);
      views = grab(/([\d,]+)\s+view/);
      bookmarks = grab(/([\d,]+)\s+bookmark/);
    }

    const isReply = /^Replying to/m.test(a.innerText);
    return { id, handle, author, time, text, replies, reposts, likes, views, bookmarks, url, is_reply: isReply };
  });
}
"""


# X/Twitter is the only site this reader is allowed to drive: it is advertised
# and permission-gated as an X reader and reuses the signed-in browser, so a full
# URL must point at X, never an arbitrary site. Accept x.com / twitter.com and any
# subdomain (mobile., www., pro., ...).
def _is_x_host(host: str) -> bool:
    host = host.lower().rsplit(":", 1)[0]
    return host in ("x.com", "twitter.com") or host.endswith((".x.com", ".twitter.com"))


def _resolve(source: str | None) -> str:
    """Turn a friendly ``source`` into the x.com URL to open.

    ``None`` is the home timeline. A full ``http(s)`` URL is used as-is, but only
    if it points at X/Twitter (this reader is gated to X and drives the signed-in
    browser, so it refuses to navigate anywhere else). A bare keyword (``"home"``,
    ``"notifications"``, ...) maps to its timeline. ``"@name"`` is a profile;
    ``"#tag"`` or any other text is a search.
    """

    if not source:
        return _NAMED["home"]
    s = source.strip()
    if s.startswith(("http://", "https://")):
        from urllib.parse import urlparse
        host = urlparse(s).hostname or ""
        if not _is_x_host(host):
            raise ValueError(
                f"x.posts only reads X/Twitter URLs, not {host!r}. Pass an "
                "x.com/twitter.com link, a \"@handle\", a \"#tag\" or search "
                "text, or a timeline keyword like \"home\"."
            )
        return s
    low = s.lower()
    if low in _NAMED:
        return _NAMED[low]
    if s.startswith("@"):
        return f"https://x.com/{s[1:]}"
    if s.startswith("#"):
        from urllib.parse import quote
        return f"https://x.com/search?q={quote(s)}&src=typed_query&f=live"
    # A plain word that looks like a single handle (no spaces) is still ambiguous
    # with a one-word search; treat free text as a search, which is the safe, most
    # useful default. Use "@name" to force a profile.
    from urllib.parse import quote
    return f"https://x.com/search?q={quote(s)}&src=typed_query&f=live"


async def posts(
    source: str | None = None,
    *,
    limit: int = 30,
    scroll: bool = True,
    endpoint: str = DEFAULT_ENDPOINT,
    app: str = DEFAULT_APP,
    timeout: float = 30.0,
) -> pl.DataFrame:
    """Recent X posts from ``source`` as a polars DataFrame, in page order.

    ``source`` is a timeline keyword (``"home"`` -- the default -- or
    ``"notifications"`` / ``"bookmarks"`` / ``"explore"``), a profile (``"@handle"``),
    a search (``"#tag"`` or any free text), or a full ``x.com`` URL (including a
    ``/status/`` thread). The browser is scrolled until ``limit`` posts are loaded
    (pass ``scroll=False`` to read only what is initially on screen).

    Columns: ``id``, ``handle``, ``author``, ``time`` (UTC datetime), ``text``,
    ``replies`` / ``reposts`` / ``likes`` / ``views`` / ``bookmarks`` (Int64,
    null when X does not show that count), ``url`` (permalink), ``is_reply``.

    Drives the browser :mod:`browser` connects to (``endpoint``), launching ``app``
    if nothing is listening. Reading a signed-in timeline needs that browser signed
    in to X. Returns an empty (correctly typed) frame when no posts are found.

    Reads the signed-in account's personal feed, so it is confined to incognito
    sessions: in a shared (multiplayer) room (``IX_MCP_SHARED`` set) it raises
    rather than leak one person's timeline into shared state.
    """

    _require_incognito()
    url = _resolve(source)
    await browser.get_or_create_browser(endpoint=endpoint, app=app, timeout=timeout)
    # Open a dedicated tab (``new=True``) rather than reusing the shared front
    # tab: the runtime runs jobs concurrently, so two reads sharing one tab would
    # clobber each other's navigation and scroll. Acquire the tab *before*
    # navigating so the ``finally`` below closes it even when ``goto`` itself
    # fails (a navigation timeout would otherwise leak the tab).
    page = await browser.page(endpoint=endpoint, new=True)

    try:
        await page.goto(url, wait_until="domcontentloaded", timeout=timeout * 1000)
        try:
            await page.wait_for_selector("article", timeout=timeout * 1000)
        except Exception:
            # No tweets rendered (an empty timeline, a login wall, a deleted
            # post): return an empty, correctly-typed frame rather than raise.
            return pl.DataFrame(schema=_SCHEMA)

        collected: dict[str, dict] = {}
        order: list[str] = []
        stable = 0
        max_passes = 60 if scroll else 1
        for _ in range(max_passes):
            before = len(collected)
            for row in await page.evaluate(_EXTRACT_JS):
                # Key on the permalink id; fall back to a synthetic key for the
                # rare promoted/edge card without one so it is not silently dropped.
                key = row.get("id") or f"{row.get('handle')}:{row.get('text')[:40]}"
                if key not in collected:
                    order.append(key)
                collected[key] = row
            if len(collected) >= limit or not scroll:
                break
            # Stop once a few scrolls in a row load nothing new (the end of the
            # timeline, or X has stopped loading more). `before` is the count from
            # before this pass's extraction, so the comparison reflects what the
            # previous scroll actually loaded, not the rows already in hand.
            stable = stable + 1 if len(collected) == before else 0
            if stable >= 3:
                break
            await page.evaluate("window.scrollBy(0, window.innerHeight * 1.5)")
            await asyncio.sleep(0.8)

        rows = [collected[k] for k in order][:limit]
        if not rows:
            return pl.DataFrame(schema=_SCHEMA)

        df = pl.DataFrame(
            rows, schema_overrides={k: v for k, v in _SCHEMA.items() if k != "time"}
        )
        return df.with_columns(
            pl.col("time").str.to_datetime(time_zone="UTC", strict=False)
        ).select(list(_SCHEMA))
    finally:
        try:
            await page.close()
        except Exception:
            pass
