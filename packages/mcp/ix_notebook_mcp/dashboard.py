"""Slim ix-mcp data API.

The human UI lives in Weave Constellation. This process keeps only the execution
endpoint and tier-3 mailbox HTTP glue needed by kernels, resources, and MCP
transport pumps.
"""

from __future__ import annotations

import asyncio
import hmac
import json
import os

from aiohttp import web

from . import mailbox, store
from .config import Config

_MAX_INPUT_BYTES = 256 * 1024
_CORS_HEADERS = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Max-Age": "86400",
}
_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1", "localhost", ""})


def landing_html() -> str:
    weave_url = os.environ.get("WEAVE_URL", "http://127.0.0.1:7677")
    return (
        "<!doctype html><meta charset=utf-8><title>ix-mcp data API</title>"
        "<style>body{font:15px/1.6 ui-monospace,monospace;max-width:44rem;margin:4rem auto;padding:0 1rem;color:#ddd;background:#111}a{color:#6cf}</style>"
        "<h1>ix-mcp data API</h1>"
        "<p>This server now exposes only the notebook data API. Open the Weave Constellation UI instead.</p>"
        f"<p><a href=\"{weave_url}\">{weave_url}</a></p>"
    )


def build_app(config: Config, db: store.AsyncConn | None = None, mb: mailbox.Mailbox | None = None) -> web.Application:
    app = web.Application()
    box = mb or mailbox.get_mailbox()

    async def index(_request: web.Request) -> web.Response:
        return web.Response(text=landing_html(), content_type="text/html")

    async def exec_run(request: web.Request) -> web.Response:
        token = config.exec_token
        trust = config.exec_trust_network and config.host not in _LOOPBACK_HOSTS
        if not token and not trust:
            return web.json_response({"error": "exec endpoint disabled (set IX_MCP_EXEC_TRUST_NETWORK on a non-loopback bind, or IX_MCP_EXEC_TOKEN)"}, status=403)
        if token and not hmac.compare_digest(request.headers.get("Authorization", ""), f"Bearer {token}"):
            return web.json_response({"error": "unauthorized"}, status=401)
        try:
            body = await request.json()
        except Exception:
            return web.json_response({"error": "body must be JSON"}, status=400)
        code = body.get("code")
        if not isinstance(code, str) or not code.strip():
            return web.json_response({"error": "missing 'code'"}, status=400)
        raw_budget = body.get("budget", 15.0)
        if isinstance(raw_budget, bool) or not isinstance(raw_budget, (int, float)):
            return web.json_response({"error": "'budget' must be a number"}, status=400)
        budget = min(max(0.0, float(raw_budget)), config.max_budget)
        from .kernel import current_kernel
        outs, summary = await current_kernel().python_exec(code, budget=budget)
        if summary is None:
            text = "".join(o.get("text", "") for o in outs if isinstance(o, dict))
            return web.json_response({"output": text, "result": None, "error": None})
        return web.json_response({"output": summary.get("output", ""), "result": summary.get("result"), "error": summary.get("error"), "status": summary.get("status")})

    async def input_preflight(_request: web.Request) -> web.Response:
        return web.Response(status=204, headers=_CORS_HEADERS)

    async def input_submit(request: web.Request) -> web.Response:
        if config.host not in _LOOPBACK_HOSTS and not config.exec_trust_network:
            return web.json_response({"error": "input endpoint disabled on this non-loopback bind (set IX_MCP_EXEC_TRUST_NETWORK to accept input over the tailnet)"}, status=403, headers=_CORS_HEADERS)
        raw = await request.read()
        if len(raw) > _MAX_INPUT_BYTES:
            return web.json_response({"error": f"payload exceeds {_MAX_INPUT_BYTES} bytes"}, status=413, headers=_CORS_HEADERS)
        try:
            body = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return web.json_response({"error": "body must be JSON"}, status=400, headers=_CORS_HEADERS)
        channel = body.get("channel") if isinstance(body, dict) else None
        if not isinstance(channel, str) or not channel:
            return web.json_response({"error": "missing 'channel'"}, status=400, headers=_CORS_HEADERS)
        if "payload" not in body:
            return web.json_response({"error": "missing 'payload'"}, status=400, headers=_CORS_HEADERS)
        if not box.channel_open(channel):
            return web.json_response({"error": "no such open channel"}, status=404, headers=_CORS_HEADERS)
        try:
            box.add_input(channel=channel, payload=json.dumps(body["payload"]))
        except ValueError as exc:
            return web.json_response({"error": str(exc)}, status=413, headers=_CORS_HEADERS)
        return web.json_response({"ok": True}, headers=_CORS_HEADERS)

    async def resource_events(request: web.Request) -> web.StreamResponse:
        rid = request.match_info["id"]
        resp = web.StreamResponse(headers={"Content-Type": "text/event-stream", "Cache-Control": "no-cache", **_CORS_HEADERS})
        await resp.prepare(request)
        await resp.write(b": connected\n\n")
        last = box.latest_event_seq(rid)
        try:
            while True:
                for row in box.events_after(rid, last):
                    last = row["seq"]
                    try:
                        body = json.loads(row["body"])
                    except (ValueError, TypeError):
                        body = {"raw": row["body"]}
                    await resp.write(f"data: {json.dumps({'seq': row['seq'], 'kind': row['kind'], **body})}\n\n".encode())
                await asyncio.sleep(0.5)
        except (ConnectionError, asyncio.CancelledError):
            pass
        return resp

    async def mailbox_outbox(request: web.Request) -> web.Response:
        body = await request.json()
        box.add_outbox(content=str(body.get("content", "")), meta=str(body.get("meta", "{}")), session=str(body.get("session", "")))
        return web.json_response({"ok": True})

    async def mailbox_inputs(request: web.Request) -> web.Response:
        rows = box.pending_inputs()
        if request.query.get("consume") in ("1", "true", "yes"):
            box.delete_inputs([row["seq"] for row in rows])
        return web.json_response(rows)

    async def mailbox_inputs_delete(request: web.Request) -> web.Response:
        body = await request.json()
        box.delete_inputs([int(s) for s in body.get("seqs", [])])
        return web.json_response({"ok": True})

    async def mailbox_events(request: web.Request) -> web.Response:
        body = await request.json()
        box.add_event(resource=str(body["resource"]), kind=str(body["kind"]), body=str(body["body"]))
        return web.json_response({"ok": True})

    async def mailbox_channels(request: web.Request) -> web.Response:
        body = await request.json()
        op = body.get("op")
        if op == "open":
            box.open_channel(id=str(body["id"]), title=str(body.get("title", "")))
        elif op == "close":
            box.close_channel(id=str(body["id"]))
        else:
            return web.json_response({"error": "op must be open or close"}, status=400)
        return web.json_response({"ok": True})

    async def mailbox_channel_open(request: web.Request) -> web.Response:
        return web.json_response({"open": box.channel_open(request.match_info["id"])})

    async def mailbox_reset(_request: web.Request) -> web.Response:
        box.reset()
        return web.json_response({"ok": True})

    async def job_ui(request: web.Request) -> web.Response:
        # MCP Apps surface (mcp_ui.py, unchanged by the Weave cutover): tool
        # results reference this URL for the interactive job view; the job
        # itself now comes from Weave facts through the store facade.
        if db is None:
            return web.json_response({"error": "no store"}, status=503)
        job = await db.run(store.get, request.match_info["id"])
        if job is None:
            return web.json_response({"error": "no such job"}, status=404)
        from . import mcp_ui

        html = mcp_ui.embedded_html(mcp_ui.job_payload(job))
        return web.Response(text=html, content_type="text/html")

    app.router.add_get("/", index)
    app.router.add_post("/api/exec", exec_run)
    app.router.add_post("/api/input", input_submit)
    app.router.add_route("OPTIONS", "/api/input", input_preflight)
    app.router.add_get("/api/resources/{id}/events", resource_events)
    app.router.add_get("/api/jobs/{id}/ui", job_ui)
    app.router.add_post("/api/mailbox/outbox", mailbox_outbox)
    app.router.add_get("/api/mailbox/inputs", mailbox_inputs)
    app.router.add_post("/api/mailbox/inputs/delete", mailbox_inputs_delete)
    app.router.add_post("/api/mailbox/events", mailbox_events)
    app.router.add_post("/api/mailbox/channels", mailbox_channels)
    app.router.add_get("/api/mailbox/channels/{id}", mailbox_channel_open)
    app.router.add_post("/api/mailbox/reset", mailbox_reset)
    return app


async def start(config: Config) -> web.AppRunner:
    db = store.AsyncConn(config.store_path)
    app = build_app(config, db)

    async def _close_store(_app: web.Application) -> None:
        await db.close()

    app.on_cleanup.append(_close_store)
    runner = web.AppRunner(app)
    await runner.setup()
    await web.TCPSite(runner, config.host, config.dashboard_port).start()
    return runner
