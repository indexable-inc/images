"""Serve the MCP tool surface over a transport.

stdio is the transport our clients launch, and the delicate one: the Jupyter
Server shares this process and would log to fd 1, corrupting the JSON-RPC stream.
The CLI dups the real stdin/stdout to private fds and points fd 0/1 at
/dev/null and stderr before the server starts, so here we hand the MCP protocol
those private fds exclusively (the only reason we reach the low-level server: its
`run` takes explicit streams, which FastMCP's stdio runner does not expose).
"""

from __future__ import annotations

import os

import anyio

from .config import config
from .tools import mcp


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
    async with stdio_server(stdin, stdout) as (read_stream, write_stream):
        await server.run(read_stream, write_stream, server.create_initialization_options())


async def _serve_http() -> None:
    cfg = config()
    mcp.settings.host = cfg.mcp_http_host
    mcp.settings.port = cfg.mcp_http_port
    await mcp.run_streamable_http_async()
