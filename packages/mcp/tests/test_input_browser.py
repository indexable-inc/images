"""End-to-end proof of the interactive-input path through a REAL headless browser.

The unit tests (``test_inputs.py``) cover the store queue, the ``/api/input``
gate, and the kernel drain in isolation. This test closes the one assumption
they cannot: that an interactive resource's injected ``ixSubmit`` -- running
inside a sandboxed, opaque-origin ``srcdoc`` iframe exactly as the dashboard's
``HtmlBody.svelte`` mounts it (``sandbox="allow-scripts"``, no
allow-same-origin) -- can actually reach the data API with a cross-origin
``fetch`` and land a submission. It stands up the real aiohttp data API, renders
a real ``Input``'s HTML in a real Chromium-hosted sandboxed iframe, clicks the
button, and asserts the payload arrives and drains into the awaiting channel.
"""

from __future__ import annotations

import asyncio
import socket
from pathlib import Path

import pytest
from aiohttp import web

from ix_notebook_mcp import dashboard, runtime, store
from ix_notebook_mcp.config import Config


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def test_sandboxed_iframe_fetch_delivers_input(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    pytest.importorskip("playwright")
    from playwright.async_api import async_playwright

    async def run() -> None:
        # The real data API over a real loopback socket.
        db = tmp_path / "browser.db"
        conn = store.connect(db)
        port = _free_port()
        base = f"http://127.0.0.1:{port}"
        cfg = Config(workdir=tmp_path, store_path=db, host="127.0.0.1", dashboard_port=port)
        runner = web.AppRunner(dashboard.build_app(cfg, conn))
        await runner.setup()
        await web.TCPSite(runner, "127.0.0.1", port).start()

        # The kernel side shares the same store and learns the endpoint the way
        # the CLI wires it (IX_MCP_DATA_API_URL -> Input.script).
        monkeypatch.setenv("IX_MCP_DATA_API_URL", base)
        monkeypatch.setattr(runtime, "_store", store)
        monkeypatch.setattr(runtime, "_store_conn", conn)
        runtime.input_channels.clear()
        inp = runtime.Input(title="play")
        body = (
            inp.script
            + "<button id='go' onclick='ixSubmit({clicked:true,who:&quot;ada&quot;})'>Go</button>"
        )

        try:
            async with async_playwright() as p:
                browser = await p.chromium.launch()
                try:
                    page = await browser.new_page()
                    # Mount the body exactly like HtmlBody.svelte: a sandboxed,
                    # opaque-origin srcdoc iframe. Assigning .srcdoc via JS avoids
                    # hand-escaping the HTML into an attribute.
                    await page.set_content('<iframe id="f" sandbox="allow-scripts"></iframe>')
                    await page.eval_on_selector("#f", "(el, html) => { el.srcdoc = html; }", body)
                    await page.frame_locator("#f").locator("#go").click()

                    # The cross-origin fetch must land a row server-side.
                    for _ in range(100):
                        if store.pending_inputs(conn):
                            break
                        await asyncio.sleep(0.05)
                    assert store.pending_inputs(conn), "submission never reached /api/input"
                finally:
                    await browser.close()

            # And the kernel drain delivers it to the awaiting channel.
            runtime._drain_inputs()
            got = await asyncio.wait_for(inp.recv(), timeout=2.0)
            assert got == {"clicked": True, "who": "ada"}
        finally:
            await runner.cleanup()

    asyncio.run(run())
