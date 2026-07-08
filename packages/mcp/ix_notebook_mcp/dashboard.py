"""The read-only data API over the execution store.

Auto-started by the CLI. It serves the live execution log as JSON
(``/api/jobs``, ``/api/jobs/{id}``, ``/api/resources``, ``/api/cells``,
``/api/snapshot``), one run's interactive HTML view (``/api/jobs/{id}/ui``,
the same MCP Apps view claude.ai renders, for the room's sandboxed iframe),
plus two write paths: the tailnet-gated ``/api/exec`` an
embedder polls (the room server reads ``/api/snapshot``; a peer's
``fleet.in_kernel`` drives ``/api/exec``), and ``/api/input``, which an
interactive resource's ``ixSubmit`` posts the human's reply to (authorized by the
network boundary like ``/api/exec``: loopback, or a trusted tailnet; see
:class:`runtime.Input`).

The human-facing UI is no longer served here: the MCP publishes its runs,
resources, and live namespace as Loro panes to the shared ``dashboard`` hub
(see :mod:`ix_notebook_mcp.pane_bridge`), which a human starts once with
``ix-mcp dashboard``. ``/`` redirects to that hub when one is running, and
otherwise serves a short page naming the command -- never a dead redirect.
"""

from __future__ import annotations

import asyncio
import hmac
import json

from aiohttp import web

from . import feed, store
from .config import Config, live_hub, port_open

# Cap on one input submission's body. A submission is a small form payload (a
# name, a choice, a few fields), never a file upload, so a generous-but-bounded
# limit keeps a hostile tailnet peer from growing the store with one giant POST.
_MAX_INPUT_BYTES = 256 * 1024

# CORS for the input write path. An interactive resource renders inside a
# sandboxed, opaque-origin iframe (``sandbox="allow-scripts"``, no
# allow-same-origin), so its `fetch` carries ``Origin: null`` and the browser
# blocks reading the response unless we allow it. We allow any origin: the
# endpoint authorizes by the unguessable channel-id capability in the body, not
# by origin, and the data API only binds the trust boundary (loopback/tailnet).
_CORS_HEADERS = {
    "Access-Control-Allow-Origin": "*",
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Max-Age": "86400",
}


def landing_html() -> str:
    """The page ``/`` serves when no shared dashboard hub is running. It is a data
    API, not the UI, so it points the human at the one command that opens the UI
    rather than (as before) redirecting to a hub port that may be dead."""
    return (
        "<!doctype html><meta charset=utf-8>"
        "<title>ix-mcp data API</title>"
        "<style>body{font:15px/1.6 ui-monospace,monospace;max-width:40rem;"
        "margin:4rem auto;padding:0 1rem;color:#ddd;background:#111}"
        "code{background:#222;padding:.1rem .35rem;border-radius:4px}"
        "a{color:#6cf}</style>"
        "<h1>ix-mcp data API</h1>"
        "<p>This is the read-only <b>data API</b> for one session, not the "
        "dashboard UI. The UI is a single shared board across every session.</p>"
        "<p>Open it with:</p>"
        "<p><code>ix-mcp dashboard</code></p>"
        "<p>Machine endpoints: "
        "<a href=/api/jobs>/api/jobs</a> · "
        "<a href=/api/resources>/api/resources</a> · "
        "<a href=/api/snapshot>/api/snapshot</a></p>"
    )

# Binds for which "trust the network" must NOT relax the exec token: a loopback
# bind is local-only, so tailnet trust is meaningless and could only mask a
# misconfiguration. Trust-network is honored only on a real (tailnet/LAN) bind.
_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1", "localhost", ""})


def build_app(config: Config, db: store.AsyncConn) -> web.Application:
    """Assemble the data API over the store's async facade ``db``.

    Split out of :func:`start` so the routes (notably the token-gated
    ``/api/exec`` write path) are testable with an in-memory app and a fake
    kernel, without binding a socket. Every store read goes through ``db`` so a
    slow read (a fat store's output blobs) costs latency on its request only,
    never the shared event loop the MCP transport runs on (index#2348)."""
    app = web.Application()

    async def index(_request: web.Request) -> web.Response:
        # The UI is the shared Loro hub. Redirect there only when one is actually
        # running (a human ran `ix-mcp dashboard`); otherwise serve a page naming
        # that command. Never redirect to `config.hub_url()` -- that per-session
        # `hub_port` is a free port reserved at startup but never bound here, so a
        # bookmark to `/` used to 302 straight into a refused connection.
        # `live_hub` does a blocking TCP probe, so run it off the shared event
        # loop (the kernel, MCP transport, and this API all run on it -- a
        # synchronous socket call here would freeze every concurrent job).
        hub = await asyncio.to_thread(live_hub)
        if hub and hub.get("url"):
            raise web.HTTPFound(hub["url"])
        # IX_MCP_AUTO_DASHBOARD spawns a per-server hub at `config.hub_port` but
        # writes no shared state, so fall back to redirecting there when it is up.
        # Gate strictly on `auto_dashboard`: in the default mode `hub_port` is a
        # reserved-but-unbound ephemeral port, and an unrelated process could later
        # reuse it -- probing it then would 302 to that wrong service.
        if config.auto_dashboard and config.hub_port:
            probe = "127.0.0.1" if config.host in ("0.0.0.0", "::", "") else config.host  # noqa: S104 -- wildcard mapped to a probeable loopback
            if await asyncio.to_thread(port_open, config.hub_port, probe):
                raise web.HTTPFound(config.hub_url())
        return web.Response(text=landing_html(), content_type="text/html")

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(await db.run(store.recent, limit=feed.JOBS_LIMIT))

    async def resources(_request: web.Request) -> web.Response:
        return web.json_response(await db.run(store.live_resources))

    async def cells(_request: web.Request) -> web.Response:
        return web.json_response(await db.run(store.cells))

    async def snapshot(_request: web.Request) -> web.Response:
        # The whole presentation in one read, the embed contract an external
        # consumer (the room server) polls; `rev` lets it skip unchanged renders.
        return web.json_response(await db.run(feed.snapshot))

    async def job(request: web.Request) -> web.Response:
        # One execution by id: the rich outputs for the `jobs['<id>']` a
        # python_exec tool result already names, so an embedder renders that run's
        # tables/plots/HTML beside the tool call.
        one = await db.run(feed.job, request.match_info["id"])
        if one is None:
            return web.json_response({"error": "no such job"}, status=404)
        return web.json_response(one)

    async def job_ui(request: web.Request) -> web.Response:
        # One execution as the same self-contained interactive HTML view an MCP
        # Apps host (claude.ai) renders for the tool result, with the payload
        # baked in (see mcp_ui.embedded_html). The room proxies this and mounts
        # it in a sandboxed (allow-scripts, opaque-origin) iframe, so the chat
        # shows the identical view without talking MCP. Read-only, derived
        # entirely from the store, like every other GET here.
        from . import mcp_ui

        one = await db.run(feed.job, request.match_info["id"])
        if one is None:
            return web.json_response({"error": "no such job"}, status=404)
        html = mcp_ui.embedded_html(mcp_ui.job_payload(one))
        return web.Response(text=html, content_type="text/html")

    async def exec_run(request: web.Request) -> web.Response:
        # The one *write* path on this otherwise read-only surface: run a line of
        # code in THIS node's live kernel so a peer's `fleet.in_kernel` can read
        # this node's real running state (its `jobs`, a held variable, hostname).
        # Two ways to gate it: a shared bearer token, and/or trusting the bound
        # network (the tailnet) as the boundary -- the same model Ray's own data
        # plane uses (any tailnet peer can already drive the Ray cluster). A token,
        # if set, is always required (defense in depth); trust-network alone is
        # honored only on a non-loopback bind. Neither -> disabled (safe default).
        token = config.exec_token
        trust = config.exec_trust_network and config.host not in _LOOPBACK_HOSTS
        if not token and not trust:
            return web.json_response(
                {
                    "error": "exec endpoint disabled (set IX_MCP_EXEC_TRUST_NETWORK "
                    "on a non-loopback bind, or IX_MCP_EXEC_TOKEN)"
                },
                status=403,
            )
        if token:
            presented = request.headers.get("Authorization", "")
            expected = f"Bearer {token}"
            # Constant-time compare so a wrong token cannot be guessed by timing.
            if not hmac.compare_digest(presented, expected):
                return web.json_response({"error": "unauthorized"}, status=401)
        try:
            body = await request.json()
        except Exception:
            return web.json_response({"error": "body must be JSON"}, status=400)
        code = body.get("code")
        if not isinstance(code, str) or not code.strip():
            return web.json_response({"error": "missing 'code'"}, status=400)
        # `bool` is an int subclass, so exclude it explicitly; clamp to
        # [0, max_budget] so a bad/negative budget is a clean 400 or a sane value
        # rather than an unhandled ValueError (a 500) for a malformed request.
        raw_budget = body.get("budget", 15.0)
        if isinstance(raw_budget, bool) or not isinstance(raw_budget, (int, float)):
            return web.json_response({"error": "'budget' must be a number"}, status=400)
        budget = min(max(0.0, float(raw_budget)), config.max_budget)
        from .kernel import current_kernel

        _outputs, summary = await current_kernel().python_exec(code, budget=budget)
        if summary is None:
            text = "".join(
                o.get("text", "") for o in _outputs if isinstance(o, dict)
            )
            return web.json_response({"output": text, "result": None, "error": None})
        return web.json_response(
            {
                "output": summary.get("output", ""),
                "result": summary.get("result"),
                "error": summary.get("error"),
                "status": summary.get("status"),
            }
        )

    async def input_preflight(_request: web.Request) -> web.Response:
        # The browser preflights any non-simple cross-origin POST. The injected
        # `ixSubmit` sends `text/plain` to stay a simple request (no preflight),
        # but answer OPTIONS too so a producer that posts `application/json`
        # itself still works.
        return web.Response(status=204, headers=_CORS_HEADERS)

    async def input_submit(request: web.Request) -> web.Response:
        # The browser -> kernel write path behind interactive resources: append a
        # DATA payload (never code) to a channel the agent explicitly opened.
        #
        # Authorization is the NETWORK boundary, not a secret. The channel id is an
        # address, not a capability: it is embedded in the resource HTML, which the
        # read endpoints (`/api/resources`, `/api/snapshot`) and the dashboard hub
        # serve to anyone who can reach the bind, so it cannot be treated as a
        # secret. Instead, input is a write governed like `/api/exec`: allowed on a
        # loopback bind (the local user is trusted, and input is low-risk), or on a
        # non-loopback (tailnet) bind only when the operator trusts the tailnet
        # (`IX_MCP_EXEC_TRUST_NETWORK`, which the fleet sets); otherwise refused.
        # The channel-open check then prevents queueing input no awaiter reads.
        if config.host not in _LOOPBACK_HOSTS and not config.exec_trust_network:
            return web.json_response(
                {
                    "error": "input endpoint disabled on this non-loopback bind "
                    "(set IX_MCP_EXEC_TRUST_NETWORK to accept input over the tailnet)"
                },
                status=403,
                headers=_CORS_HEADERS,
            )
        raw = await request.read()
        if len(raw) > _MAX_INPUT_BYTES:
            return web.json_response(
                {"error": f"payload exceeds {_MAX_INPUT_BYTES} bytes"},
                status=413,
                headers=_CORS_HEADERS,
            )
        try:
            body = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return web.json_response(
                {"error": "body must be JSON"}, status=400, headers=_CORS_HEADERS
            )
        channel = body.get("channel") if isinstance(body, dict) else None
        if not isinstance(channel, str) or not channel:
            return web.json_response(
                {"error": "missing 'channel'"}, status=400, headers=_CORS_HEADERS
            )
        if "payload" not in body:
            return web.json_response(
                {"error": "missing 'payload'"}, status=400, headers=_CORS_HEADERS
            )
        if not await db.run(store.channel_open, channel):
            # Unknown or already-closed channel: refuse rather than queue input no
            # awaiter will ever read (and so an attacker cannot grow the table by
            # posting to random ids).
            return web.json_response(
                {"error": "no such open channel"}, status=404, headers=_CORS_HEADERS
            )
        await db.run(store.add_input, channel=channel, payload=json.dumps(body["payload"]))
        return web.json_response({"ok": True}, headers=_CORS_HEADERS)

    async def resource_events(request: web.Request) -> web.StreamResponse:
        # The live event feed behind an interactive resource: action results and
        # errors (kernel handlers) plus the agent's `reply` messages, streamed as
        # SSE so the resource's page updates without polling. A read endpoint
        # like /api/resources (which already serves the same resource's HTML to
        # anyone on the bind), so no extra gate; CORS because the subscriber is
        # the sandboxed, opaque-origin iframe. Subscribers start at the CURRENT
        # tail (no history replay): the feed is a live stream, not a log.
        rid = request.match_info["id"]
        resp = web.StreamResponse(
            headers={
                "Content-Type": "text/event-stream",
                "Cache-Control": "no-cache",
                **_CORS_HEADERS,
            }
        )
        await resp.prepare(request)
        # A comment frame so EventSource sees bytes immediately (open fires).
        await resp.write(b": connected\n\n")
        last = await db.run(store.latest_event_seq, rid)
        try:
            while True:
                for row in await db.run(store.events_after, rid, last):
                    last = row["seq"]
                    try:
                        body = json.loads(row["body"])
                    except (ValueError, TypeError):
                        body = {"raw": row["body"]}
                    event = {"seq": row["seq"], "kind": row["kind"], **body}
                    await resp.write(f"data: {json.dumps(event)}\n\n".encode())
                await asyncio.sleep(0.5)
        except (ConnectionError, asyncio.CancelledError):
            # Subscriber went away (tab closed, dashboard reloaded): just stop.
            pass
        return resp

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    app.router.add_get("/api/jobs/{id}", job)
    app.router.add_get("/api/jobs/{id}/ui", job_ui)
    app.router.add_get("/api/resources", resources)
    app.router.add_get("/api/resources/{id}/events", resource_events)
    app.router.add_get("/api/cells", cells)
    app.router.add_get("/api/snapshot", snapshot)
    app.router.add_post("/api/exec", exec_run)
    app.router.add_post("/api/input", input_submit)
    app.router.add_route("OPTIONS", "/api/input", input_preflight)
    return app


async def start(config: Config) -> web.AppRunner:
    # `config.host` is resolved to a bindable address by the CLI before the
    # kernel spawns (see cli._serve), so the bind here is expected to succeed;
    # a failure is a genuine error worth surfacing.
    db = store.AsyncConn(config.store_path)
    app = build_app(config, db)

    async def _close_store(_app: web.Application) -> None:
        await db.close()

    app.on_cleanup.append(_close_store)
    runner = web.AppRunner(app)
    await runner.setup()
    await web.TCPSite(runner, config.host, config.dashboard_port).start()
    return runner
