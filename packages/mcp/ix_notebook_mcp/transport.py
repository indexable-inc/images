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
import json
import logging
import os
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
    """Drain the store's outbox into ``notifications/claude/channel`` events.

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
    conn = store.connect(cfg.store_path)
    try:
        while True:
            # Wait for the handshake before draining, so rows that accrue during
            # startup are held (not dropped) until the client can receive them.
            if not _session_initialized(session):
                await anyio.sleep(_OUTBOX_POLL_SECONDS)
                continue
            try:
                rows = store.take_outbox(conn)
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
        conn.close()


async def _serve_http() -> None:
    cfg = config()
    mcp.settings.host = cfg.mcp_http_host
    mcp.settings.port = cfg.mcp_http_port
    await mcp.run_streamable_http_async()
