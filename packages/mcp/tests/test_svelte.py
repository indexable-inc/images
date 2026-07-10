"""The Svelte resource environment, compile to click, through a REAL browser.

``svelte.bundle`` shells out to the nix-built ``svelte-bundle`` CLI
(``IX_SVELTE_BUNDLE_BIN``), so these tests skip when it is not wired up (plain
`pytest` outside the nix sandbox without the wrapper env). What they close that
no unit test can: a Svelte 5 component compiled by our CLI, mounted in the same
sandboxed opaque-origin iframe the dashboard uses, renders the kernel-embedded
initial state, sends ``ix.act`` through the REAL ``/api/input``, and reactively
re-renders from the ``action_result`` the real dispatcher emits.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
from pathlib import Path

import pytest
from aiohttp import web

from ix_notebook_mcp import dashboard, runtime, store
from ix_notebook_mcp.config import Config

import svelte

pytestmark = pytest.mark.skipif(
    not os.environ.get("IX_SVELTE_BUNDLE_BIN"),
    reason="IX_SVELTE_BUNDLE_BIN not set (svelte-bundle CLI not wired up)",
)

COUNTER = """
<script>
  import { data, act } from 'ix';
</script>
<h1 id="count">{$data.count}</h1>
<button id="bump" onclick={() => act('bump', { by: 1 })}>+1</button>
<style>
  h1 { color: rgb(158, 206, 106); }
</style>
"""


# State-only: imports `data` but wires no actions, so the resource HTML
# carries no window.ix wiring script at all.
DISPLAY = """
<script>
  import { data } from 'ix';
</script>
<p id="msg">{$data.msg}</p>
"""


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def test_bundle_compiles_inline_component() -> None:
    js = asyncio.run(svelte.bundle(COUNTER))
    assert "__IX_STATE__" in js  # the virtual ix module got bundled in
    safe = svelte._inline_js_safe(js)
    assert "</script" not in safe
    assert "<!--" not in safe


def test_seed_json_blocks_script_data_breakout() -> None:
    # `<!--<script>` in a state string enters script-data-double-escaped and
    # swallows the following bundle <script> if emitted verbatim (WHATWG
    # tokenizer); the seed must never contain a raw `<`.
    seed = svelte._seed_json({"note": "<!--<script></script>"})
    assert "<" not in seed
    assert json.loads(seed) == {"note": "<!--<script></script>"}


def test_bundle_reports_compile_errors() -> None:
    with pytest.raises(svelte.SvelteError, match="svelte-bundle failed"):
        asyncio.run(svelte.bundle("<h1>{unclosed</h1>"))


def test_state_only_component_mounts_inside_container(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """No actions -> no window.ix wiring: the ix module must not throw on load,
    and the app must mount inside the script's containing element (ix-windows
    measures #ix-content for native window sizing, so a body-level mount would
    render outside the measured wrapper)."""
    pytest.importorskip("playwright")
    from playwright.async_api import async_playwright

    async def run() -> None:
        conn = store.connect(tmp_path / "svelte.db")
        monkeypatch.setattr(runtime, "_store", store)
        monkeypatch.setattr(runtime, "_store_conn", conn)

        res = await svelte.component(DISPLAY, id="svelte-test-display", state={"msg": "hello"})
        body = await res.render_html()
        assert "window.ix=window.ix" not in body  # state-only: no wiring script
        wrapped = f'<div id="ix-content">{body}</div>'

        try:
            async with async_playwright() as p:
                browser = await p.chromium.launch()
                try:
                    page = await browser.new_page()
                    await page.set_content('<iframe id="f" sandbox="allow-scripts"></iframe>')
                    await page.eval_on_selector("#f", "(el, html) => { el.srcdoc = html; }", wrapped)
                    frame = page.frame_locator("#f")
                    # the guarded ix module survived the missing window.ix and
                    # rendered the kernel-embedded seed
                    assert await frame.locator("#msg").text_content() == "hello"
                    # mounted INSIDE the measured wrapper, not on document.body
                    assert await frame.locator("#ix-content #msg").count() == 1
                finally:
                    await browser.close()
        finally:
            res.close()

    asyncio.run(run())


def test_component_click_roundtrip(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pytest.importorskip("playwright")
    from playwright.async_api import async_playwright

    async def run() -> None:
        db = tmp_path / "svelte.db"
        conn = store.connect(db)
        port = _free_port()
        base = f"http://127.0.0.1:{port}"
        cfg = Config(workdir=tmp_path, store_path=db, host="127.0.0.1", dashboard_port=port)
        runner = web.AppRunner(dashboard.build_app(cfg, store.AsyncConn(cfg.store_path)))
        await runner.setup()
        await web.TCPSite(runner, "127.0.0.1", port).start()

        monkeypatch.setenv("IX_MCP_DATA_API_URL", base)
        monkeypatch.setattr(runtime, "_store", store)
        monkeypatch.setattr(runtime, "_store_conn", conn)
        runtime.input_channels.clear()

        game = {"count": 41}

        async def bump(payload: dict) -> dict:
            game["count"] += int(payload["by"])
            return dict(game)

        res = await svelte.component(
            COUNTER, id="svelte-test-counter", state=lambda: game, actions={"bump": bump}
        )
        body = await res.render_html()

        try:
            async with async_playwright() as p:
                browser = await p.chromium.launch()
                try:
                    page = await browser.new_page()
                    await page.set_content('<iframe id="f" sandbox="allow-scripts"></iframe>')
                    await page.eval_on_selector("#f", "(el, html) => { el.srcdoc = html; }", body)
                    frame = page.frame_locator("#f")

                    # kernel-embedded initial state rendered by the component
                    assert await frame.locator("#count").text_content() == "41"

                    await frame.locator("#bump").click()
                    # cross-origin fetch -> /api/input -> kernel drain -> handler
                    for _ in range(100):
                        runtime._drain_inputs()
                        if game["count"] == 42:
                            break
                        await asyncio.sleep(0.05)
                    assert game["count"] == 42, "act('bump') never reached the handler"

                    # action_result streams back over SSE; $data re-renders
                    for _ in range(100):
                        if await frame.locator("#count").text_content() == "42":
                            break
                        await asyncio.sleep(0.05)
                    assert await frame.locator("#count").text_content() == "42"

                    # injected (scoped) CSS came along in the one bundle
                    color = await frame.locator("#count").evaluate(
                        "el => getComputedStyle(el).color"
                    )
                    assert color == "rgb(158, 206, 106)"
                finally:
                    await browser.close()
        finally:
            res.close()
            await runner.cleanup()

    asyncio.run(run())
