"""Jupyter Server extension that boots the MCP server in-process.

Running as a server extension is what makes real-time co-editing possible: the
MCP tools end up in the same process as the YDoc collaboration rooms, so they can
edit the exact document a human's browser is subscribed to. When the server
finishes starting, this extension binds the live ``serverapp`` into the runtime
and launches the MCP transport as a background task on the server's event loop.
"""

from __future__ import annotations

import asyncio
import sys

from jupyter_server.extension.application import ExtensionApp

from .runtime import RUNTIME, runtime_dir


class IxNotebookMCPExtension(ExtensionApp):
    name = "ix_notebook_mcp"
    # Pure backend extension: no HTTP routes of its own in stdio mode. The MCP
    # transport is started from the post-start hook below.
    handlers: list = []  # type: ignore[assignment]

    async def _start_jupyter_server_extension(self, serverapp) -> None:
        RUNTIME.serverapp = serverapp
        # Resolve the real bound address/token now that the server is up (port 0
        # means the OS chose a free port), so the lab URL we hand out is correct.
        RUNTIME.host = serverapp.ip or RUNTIME.host
        RUNTIME.port = serverapp.port
        token = getattr(serverapp, "token", None) or RUNTIME.token
        RUNTIME.token = token

        url = RUNTIME.lab_url()
        (runtime_dir() / "lab-url").write_text(url)
        print(f"[ix-mcp] co-edit this notebook live in JupyterLab: {url}", file=sys.stderr, flush=True)

        if RUNTIME.transport == "http":
            from .serve import serve_http

            self._ix_mcp_task = asyncio.create_task(serve_http())
        else:
            from .serve import serve_stdio

            self._ix_mcp_task = asyncio.create_task(serve_stdio())
