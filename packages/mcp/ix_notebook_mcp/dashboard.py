"""The read-only data API over the execution store.

Auto-started by the CLI. It serves the live execution log as JSON
(``/api/jobs``, ``/api/jobs/{id}``, ``/api/resources``, ``/api/cells``,
``/api/snapshot``) plus the tailnet-gated ``/api/exec`` write path, which
embedders poll (the room server reads ``/api/snapshot``; a peer's
``fleet.in_kernel`` drives ``/api/exec``).

The human-facing UI is no longer served here: the MCP publishes its runs,
resources, and live namespace as Loro panes to the shared ``dashboard`` hub
(see :mod:`ix_notebook_mcp.pane_bridge`), which the CLI spawns and advertises.
``/`` redirects there so an old bookmark still lands on the live board.
"""

from __future__ import annotations

import hmac

from aiohttp import web

from . import feed, store
from .config import Config

# Binds for which "trust the network" must NOT relax the exec token: a loopback
# bind is local-only, so tailnet trust is meaningless and could only mask a
# misconfiguration. Trust-network is honored only on a real (tailnet/LAN) bind.
_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1", "localhost", ""})


def build_app(config: Config, conn) -> web.Application:
    """Assemble the data API over an open store ``conn``.

    Split out of :func:`start` so the routes (notably the token-gated
    ``/api/exec`` write path) are testable with an in-memory app and a fake
    kernel, without binding a socket."""
    app = web.Application()

    async def index(_request: web.Request) -> web.Response:
        # The UI is the Loro hub now; send a stray visitor of the old URL there.
        raise web.HTTPFound(config.hub_url())

    async def jobs(_request: web.Request) -> web.Response:
        return web.json_response(store.recent(conn, limit=feed.JOBS_LIMIT))

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

    app.router.add_get("/", index)
    app.router.add_get("/api/jobs", jobs)
    app.router.add_get("/api/jobs/{id}", job)
    app.router.add_get("/api/resources", resources)
    app.router.add_get("/api/cells", cells)
    app.router.add_get("/api/snapshot", snapshot)
    app.router.add_post("/api/exec", exec_run)
    return app


async def start(config: Config) -> web.AppRunner:
    # `config.host` is resolved to a bindable address by the CLI before the
    # kernel spawns (see cli._serve), so the bind here is expected to succeed;
    # a failure is a genuine error worth surfacing.
    conn = store.connect(config.store_path)
    app = build_app(config, conn)
    runner = web.AppRunner(app)
    await runner.setup()
    await web.TCPSite(runner, config.host, config.dashboard_port).start()
    return runner
