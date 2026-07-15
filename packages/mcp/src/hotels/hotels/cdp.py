"""Raw Chrome DevTools Protocol transport over a websocket.

The seam that lets every backend reuse the user's already-running, bot-trusted
browser instead of launching a fresh (challenge-walled) one. The bundled
:mod:`browser` module (Playwright over CDP) is the intended shared abstraction,
but its pinned driver currently cannot handshake with very recent Chrome builds,
so we speak raw CDP here through a tiny client that can be swapped for
:mod:`browser` once the versions realign.
"""

from __future__ import annotations

import asyncio
import contextlib
import itertools
import json
from typing import Any

import httpx
import websockets

DEFAULT_ENDPOINT = "http://127.0.0.1:9222"

# Hotel sites collapse their filter rail behind a hamburger at narrow widths; a
# new CDP tab has no real window, so force a wide desktop viewport so the
# full-width layout (visible filter rail, more cards) renders.
_VIEWPORT = {"width": 1440, "height": 2200, "deviceScaleFactor": 1, "mobile": False}


class BotWall(RuntimeError):
    """Raised when a site bounces the session to a login / anti-bot challenge."""


class CDP:
    """A minimal raw-CDP client: one websocket, one freshly-created tab."""

    def __init__(self, endpoint: str = DEFAULT_ENDPOINT) -> None:
        self._endpoint = endpoint
        self._ws: Any = None
        self._ids = itertools.count(1)
        self._pending: dict[int, asyncio.Future[dict[str, Any]]] = {}
        self._reader: asyncio.Task[None] | None = None
        self._target_id: str | None = None
        self._session: str | None = None

    async def __aenter__(self) -> CDP:
        await self.open()
        return self

    async def __aexit__(self, *exc: object) -> None:
        await self.close()

    async def open(self) -> None:
        # The CDP endpoint is always a local http:// debug port, so don't let a
        # broken ambient environment break the probe (matches the `browser`
        # module's hardening): verify=False avoids the default client eagerly
        # building an SSL context from a possibly-missing $SSL_CERT_FILE, and
        # trust_env=False ignores any $HTTP(S)_PROXY/$ALL_PROXY that would
        # misroute a localhost request.
        async with httpx.AsyncClient(verify=False, trust_env=False) as client:  # noqa: S501 -- local-only CDP probe, never TLS
            try:
                ver = (await client.get(f"{self._endpoint}/json/version")).json()
            except Exception as exc:
                raise RuntimeError(
                    f"no browser is listening at {self._endpoint}. Start one with "
                    "--remote-debugging-port=9222 (the port the `browser` module uses)."
                ) from exc
        # proxy=None: same reason -- never route the localhost devtools socket
        # through a system/env-configured proxy.
        self._ws = await websockets.connect(
            ver["webSocketDebuggerUrl"], max_size=None, proxy=None
        )
        self._reader = asyncio.create_task(self._read_loop())
        created = await self.send("Target.createTarget", {"url": "about:blank"})
        self._target_id = created["targetId"]
        attached = await self.send(
            "Target.attachToTarget", {"targetId": self._target_id, "flatten": True}
        )
        self._session = attached["sessionId"]
        await self.send("Page.enable", session=True)
        await self.send("Runtime.enable", session=True)
        await self.send("Emulation.setDeviceMetricsOverride", _VIEWPORT, session=True)

    async def _read_loop(self) -> None:
        with contextlib.suppress(Exception):
            async for raw in self._ws:
                msg = json.loads(raw)
                fut = self._pending.pop(msg.get("id"), None)
                if fut is not None and not fut.done():
                    fut.set_result(msg)

    async def send(
        self, method: str, params: dict[str, Any] | None = None, *,
        session: bool = False, timeout: float = 30.0,
    ) -> dict[str, Any]:
        msg_id = next(self._ids)
        msg: dict[str, Any] = {"id": msg_id, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = self._session
        fut: asyncio.Future[dict[str, Any]] = asyncio.get_event_loop().create_future()
        self._pending[msg_id] = fut
        await self._ws.send(json.dumps(msg))
        reply = await asyncio.wait_for(fut, timeout)
        if "error" in reply:
            raise RuntimeError(f"CDP {method} failed: {reply['error']}")
        return reply.get("result", {})

    async def navigate(self, url: str) -> None:
        await self.send("Page.navigate", {"url": url}, session=True)

    async def eval(self, expression: str, *, await_promise: bool = False) -> object:
        result = await self.send(
            "Runtime.evaluate",
            {"expression": expression, "returnByValue": True, "awaitPromise": await_promise},
            session=True,
        )
        # An in-page exception comes back as exceptionDetails with result.value
        # null; surface it instead of silently returning None, so a broken
        # extraction script is reported as a backend error, not empty results.
        details = result.get("exceptionDetails")
        if details:
            text = (
                (details.get("exception") or {}).get("description")
                or details.get("text")
                or "in-page JavaScript error"
            )
            raise RuntimeError(f"in-page eval failed: {text}")
        return result.get("result", {}).get("value")

    async def count(self, selector: str) -> int:
        n = await self.eval(f"document.querySelectorAll({json.dumps(selector)}).length")
        return int(n or 0)

    async def wait_for(
        self, selector: str, *, timeout: float = 30.0, bot_markers: tuple[str, ...] = (),
    ) -> int:
        """Poll until ``selector`` matches at least one node.

        Raises :class:`BotWall` on a login / challenge redirect and
        ``TimeoutError`` if the selector never appears within ``timeout`` -- a
        timeout is a load failure (DOM change, slow page, silent block), NOT a
        valid empty result, so the caller's error path can report the failed site
        rather than emit empty rows.
        """
        sel = json.dumps(selector)
        waited = 0.0
        while waited < timeout:
            # One round-trip per poll: read url, title and the match count together
            # (compare() polls 4 tabs concurrently, so the saved round-trips add up).
            href, title, n = await self.eval(
                f"[location.href, document.title, document.querySelectorAll({sel}).length]"
            ) or ["", "", 0]
            href, title = str(href or ""), str(title or "")
            blob = f"{href} {title}".lower()
            if "/login" in href or any(m.lower() in blob for m in bot_markers):
                raise BotWall(
                    f"redirected to a login / bot challenge ({title!r}). Use a "
                    "normal signed-in browser on the debug port."
                )
            if n:
                return int(n)
            await asyncio.sleep(1.0)
            waited += 1.0
        raise TimeoutError(
            f"result selector {selector!r} did not appear within {timeout:.0f}s "
            "(layout change, slow load, or silent block)"
        )

    async def scroll_until(self, selector: str, *, limit: int, rounds: int = 14) -> int:
        """Scroll to lazy-load cards until ``limit`` are present (or it stalls)."""
        last = -1
        for _ in range(rounds):
            n = await self.count(selector)
            if n >= limit or n == last:  # reached the target, or stalled (no new cards)
                break
            last = n
            await self.eval("window.scrollBy(0, window.innerHeight * 2)")
            await asyncio.sleep(1.0)
        return await self.count(selector)

    async def screenshot(self) -> bytes:
        import base64
        shot = await self.send("Page.captureScreenshot", {"format": "png"}, session=True)
        return base64.b64decode(shot["data"])

    async def close(self) -> None:
        with contextlib.suppress(Exception):
            if self._target_id is not None:
                await self.send("Target.closeTarget", {"targetId": self._target_id})
        if self._reader is not None:
            self._reader.cancel()
        with contextlib.suppress(Exception):
            if self._ws is not None:
                await self._ws.close()
