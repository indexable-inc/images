"""Serve the MCP tool surface over a transport.

stdio is the transport our clients launch: the CLI dups the real stdin/stdout to
private fds and points fd 0/1 at /dev/null and stderr before anything else can
write to them, so here we hand the MCP protocol those private fds exclusively.

The stdio transport is also a Claude Code channel (research preview,
https://code.claude.com/docs/en/channels-reference): it advertises the
``claude/channel`` experimental capability and pumps the store's ``outbox``
(what the kernel's ``notify()`` writes) as ``notifications/claude/channel``
events, so kernel code can push into the running agent session. Channels are
stdio-only by contract (Claude Code spawns the channel server as a subprocess
and a session opts in per-entry via ``--channels`` /
``--dangerously-load-development-channels``), so the HTTP transport does not
grow one. A client that did not opt in ignores both the capability and the
notifications, so this costs nothing when unused.
"""

from __future__ import annotations

import functools
import hmac
import json
import logging
import os
from collections.abc import Awaitable, Callable, MutableMapping
from contextlib import AsyncExitStack

import anyio
from anyio.abc import ObjectSendStream
from mcp.server.session import InitializationState, ServerSession
from mcp.shared.message import SessionMessage
from mcp.types import JSONRPCMessage, JSONRPCNotification

from . import store
from .config import config
from .tools import mcp

logger = logging.getLogger(__name__)

# The capability that makes this MCP server a channel: presence of the key is the
# whole contract (the value is always {}), and it is what tells Claude Code to
# register a listener for our channel notifications.
CHANNEL_CAPABILITIES = {"claude/channel": {}}

# How often the pump drains the outbox. Matches the kernel flusher's cadence:
# a notify() reaches the client within ~this bound.
_OUTBOX_POLL_SECONDS = 0.5


async def serve() -> None:
    if config().transport == "http":
        await _serve_http()
    else:
        await _serve_stdio()


async def _serve_stdio() -> None:
    from mcp.server.stdio import stdio_server

    cfg = config()
    if cfg.stdin_fd is None or cfg.stdout_fd is None:
        raise RuntimeError("stdio fds were not captured; serve must run under `ix-mcp serve`")

    stdin = anyio.wrap_file(os.fdopen(cfg.stdin_fd, "r", encoding="utf-8"))
    stdout = anyio.wrap_file(os.fdopen(cfg.stdout_fd, "w", encoding="utf-8", buffering=1))
    server = mcp._mcp_server
    init_options = server.create_initialization_options(
        experimental_capabilities=CHANNEL_CAPABILITIES,
    )
    async with stdio_server(stdin, stdout) as (read_stream, write_stream):
        await _run_with_channel_pump(server, read_stream, write_stream, init_options)


def _can_hold_session(server: object) -> bool:
    """Whether we can safely mirror ``Server.run`` to hold the live session (so the
    pump can gate on the client's ``initialized``). Guards every SDK internal the
    mirror touches; if a future SDK version drops or renames one -- or enables the
    experimental task-support path we do not model -- we fall back to the plain
    ``server.run`` (which never dropped a message; it just cannot gate the pump).
    """
    return (
        getattr(server, "_experimental_handlers", "unset") is None
        and all(hasattr(server, attr) for attr in ("lifespan", "_handle_message"))
        and hasattr(ServerSession, "incoming_messages")
    )


async def _run_with_channel_pump(server, read_stream, write_stream, init_options) -> None:  # noqa: ANN001 -- SDK stream/server types are internal
    """Run the MCP protocol loop with the channel outbox pump alongside it.

    The pump must not emit ``notifications/claude/channel`` before the client's
    ``initialized`` (the MCP lifecycle SHOULD-NOT send server notifications
    before then), so it needs the live ``ServerSession`` to check state.
    ``Server.run`` owns that session and does not expose it, so mirror its exact
    steps (lifespan -> session -> dispatch each incoming message) and hand the
    session to the pump. If the SDK internals this relies on are unavailable
    (see :func:`_can_hold_session`), fall back to the plain ``run`` with an
    ungated pump -- correct today because nothing writes the outbox before init
    (the store's outbox is cleared at startup), just not future-proofed.
    """
    if not _can_hold_session(server):
        logger.warning("channel pump: SDK internals unavailable; running without the initialized gate")
        async with anyio.create_task_group() as tg:
            tg.start_soon(pump_outbox, write_stream, None)
            await server.run(read_stream, write_stream, init_options)
            tg.cancel_scope.cancel()
        return
    async with AsyncExitStack() as stack:
        lifespan_context = await stack.enter_async_context(server.lifespan(server))
        session = await stack.enter_async_context(
            ServerSession(read_stream, write_stream, init_options)
        )
        async with anyio.create_task_group() as tg:
            tg.start_soon(pump_outbox, write_stream, session)
            async for message in session.incoming_messages:
                tg.start_soon(
                    functools.partial(
                        server._handle_message, message, session, lifespan_context, raise_exceptions=False
                    )
                )
            tg.cancel_scope.cancel()


def _session_initialized(session: ServerSession | None) -> bool:
    """Whether the client has completed the initialize handshake. None session
    (the fallback path) reports True: there is nothing to gate on there."""
    if session is None:
        return True
    return getattr(session, "_initialization_state", None) is InitializationState.Initialized


async def pump_outbox(
    write_stream: ObjectSendStream[SessionMessage],
    session: ServerSession | None,
) -> None:
    """Drain this session's share of the store outbox into
    ``notifications/claude/channel`` events (broadcast rows plus rows addressed
    to ``config().server_session_id`` -- see ``store.take_outbox``).

    Custom notification methods are not in the SDK's typed ``ServerNotification``
    union, so the JSON-RPC message is built directly and sent on the transport's
    write stream -- the same bytes ``ServerSession.send_notification`` would
    produce. Holds every send until the client is ``initialized`` (see
    :func:`_session_initialized`), so a startup/replay ``notify()`` never emits a
    notification before the handshake completes. Best-effort per tick: a store
    hiccup retries next tick, and a closed transport ends the pump (the task
    group tears it down anyway).
    """
    cfg = config()
    if not cfg.store_path:
        return
    # Through the async facade: `take_outbox` deletes as it reads, and on a fat
    # store that write used to run inline on the shared event loop (index#2348).
    db = store.AsyncConn(cfg.store_path)
    try:
        while True:
            # Wait for the handshake before draining, so rows that accrue during
            # startup are held (not dropped) until the client can receive them.
            if not _session_initialized(session):
                await anyio.sleep(_OUTBOX_POLL_SECONDS)
                continue
            try:
                # Serve only this session's mail: broadcast rows (explicit
                # notify() -- pr_watch and friends) plus rows addressed to this
                # server's own session id. A job lifecycle event addressed to
                # another session stays queued for its own pump instead of
                # waking this client (issue #2165).
                rows = await db.run(store.take_outbox, session=cfg.server_session_id)
            except Exception:
                rows = []  # best-effort: a read error this tick just retries next tick
            for row in rows:
                try:
                    meta = json.loads(row["meta"] or "{}")
                except ValueError:
                    meta = {}
                params: dict = {"content": row["content"]}
                if meta:
                    params["meta"] = meta
                notification = JSONRPCNotification(
                    jsonrpc="2.0",
                    method="notifications/claude/channel",
                    params=params,
                )
                try:
                    await write_stream.send(SessionMessage(message=JSONRPCMessage(notification)))
                except (anyio.ClosedResourceError, anyio.BrokenResourceError):
                    return
            await anyio.sleep(_OUTBOX_POLL_SECONDS)
    finally:
        await db.close()


# ASGI plumbing types for the HTTP transport's auth gate. `object` values (not
# Any) keep the wrapper honestly typed; only our own literal dicts flow through.
_Message = MutableMapping[str, object]
_Scope = MutableMapping[str, object]
_Receive = Callable[[], Awaitable[_Message]]
_Send = Callable[[_Message], Awaitable[None]]
_App = Callable[[_Scope, _Receive, _Send], Awaitable[None]]

# Liveness probe for a fronting reverse proxy / uptime monitor: always
# unauthenticated (it leaks nothing), so the prober never holds the API key.
_HEALTH_PATH = "/health"


def _request_key(scope: _Scope) -> bytes | None:
    """The API key a request presented, or None.

    ``X-Api-Key`` is the primary carrier: some agent-platform egress proxies
    strip ``Authorization`` from requests to allowlisted domains, so a
    bearer-only gate would lock those clients out. ``Authorization: Bearer`` is
    still accepted for clients whose path preserves it. When both appear,
    X-Api-Key wins.
    """
    bearer: bytes | None = None
    api_key: bytes | None = None
    headers = scope.get("headers")
    if isinstance(headers, (list, tuple)):
        for name, value in headers:
            if name == b"x-api-key":
                api_key = value
            elif name == b"authorization" and value[:7].lower() == b"bearer ":
                bearer = value[7:]
    return api_key if api_key is not None else bearer


async def _plain_response(send: _Send, status: int, body: bytes) -> None:
    await send(
        {
            "type": "http.response.start",
            "status": status,
            "headers": [
                (b"content-type", b"text/plain; charset=utf-8"),
                (b"content-length", str(len(body)).encode()),
            ],
        }
    )
    await send({"type": "http.response.body", "body": body})


def _gate(inner: _App, api_key: str | None) -> _App:
    """Wrap the streamable-HTTP app with the health probe and the API-key gate.

    ``GET/HEAD /health`` answers 200 unauthenticated. With a key configured,
    every other HTTP request must present it (see :func:`_request_key`) or is
    refused 401 before it reaches the MCP session manager; the comparison is
    constant-time. Non-HTTP scopes (lifespan) pass through untouched. With no
    key the gate is transparent -- the CLI has already confined that mode to a
    loopback/tailnet bind (`cli._http_bind_error`).
    """
    expected = api_key.encode() if api_key is not None else None

    async def app(scope: _Scope, receive: _Receive, send: _Send) -> None:
        if scope.get("type") != "http":
            await inner(scope, receive, send)
            return
        if scope.get("path") == _HEALTH_PATH and scope.get("method") in ("GET", "HEAD"):
            await _plain_response(send, 200, b"ok")
            return
        if expected is not None:
            presented = _request_key(scope)
            if presented is None or not hmac.compare_digest(presented, expected):
                await _plain_response(send, 401, b"missing or invalid API key")
                return
        await inner(scope, receive, send)

    return app


async def _serve_http() -> None:
    """Serve MCP over streamable HTTP (endpoint path: `/mcp`).

    Mirrors ``FastMCP.run_streamable_http_async`` (its streamable-HTTP ASGI app
    under uvicorn) so the API-key gate can sit between uvicorn and the session
    manager -- the SDK runner offers no hook for per-request auth.
    """
    import uvicorn

    cfg = config()
    mcp.settings.host = cfg.mcp_http_host
    mcp.settings.port = cfg.mcp_http_port
    app = _gate(mcp.streamable_http_app(), cfg.api_key)
    server = uvicorn.Server(
        uvicorn.Config(
            app,
            host=cfg.mcp_http_host,
            port=cfg.mcp_http_port,
            log_level=mcp.settings.log_level.lower(),
        )
    )
    await server.serve()
