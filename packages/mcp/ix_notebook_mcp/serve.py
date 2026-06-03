"""Serve the MCP tool surface over a transport.

stdio is the transport our clients (Claude Code, Codex) launch, and it is the
delicate one: the Jupyter Server shares this process and would happily log to
fd 1, corrupting the JSON-RPC stream. The CLI dups the real stdin/stdout to
private fds and points fd 0/1 at /dev/null and stderr before the server starts,
so here we hand the MCP protocol those private fds exclusively.
"""

from __future__ import annotations

import os

import anyio

from .runtime import RUNTIME
from .tools import mcp


async def serve_stdio() -> None:
    from mcp.server.stdio import stdio_server

    if RUNTIME.mcp_stdin_fd is None or RUNTIME.mcp_stdout_fd is None:
        raise RuntimeError("stdio fds were not captured; serve_stdio must run under `ix-mcp serve`")

    stdin = anyio.wrap_file(os.fdopen(RUNTIME.mcp_stdin_fd, "r", encoding="utf-8"))
    stdout = anyio.wrap_file(os.fdopen(RUNTIME.mcp_stdout_fd, "w", encoding="utf-8", buffering=1))

    server = mcp._mcp_server
    async with stdio_server(stdin, stdout) as (read_stream, write_stream):
        await server.run(read_stream, write_stream, server.create_initialization_options())


async def serve_http() -> None:
    # Streamable HTTP runs in the same process as the Jupyter Server, so the tools
    # still reach the live YDoc rooms. FastMCP mounts its own ASGI app on the
    # configured host/port.
    mcp.settings.host = RUNTIME.mcp_http_host
    mcp.settings.port = RUNTIME.mcp_http_port
    await mcp.run_streamable_http_async()
