"""Jupyter Server extension that boots the MCP server in-process.

Running as a server extension is what makes real-time co-editing possible: the
MCP tools end up in the same process as the YDoc collaboration rooms, so they can
edit the exact document a human's browser is subscribed to. When the server
finishes starting, this builds the :class:`~ix_notebook_mcp.app.NotebookApp` over
the live ``serverapp`` and launches the MCP transport on the server's event loop.
"""

from __future__ import annotations

import asyncio
import os
import sys

from jupyter_server.extension.application import ExtensionApp

from .app import NotebookApp, set_app
from .config import config, runtime_dir


class IxNotebookMCPExtension(ExtensionApp):
    name = "ix_notebook_mcp"
    # Pure backend extension: no HTTP routes of its own in stdio mode. The MCP
    # transport is started from the post-start hook below.
    handlers: list = []  # type: ignore[assignment]

    async def _start_jupyter_server_extension(self, serverapp) -> None:
        set_app(NotebookApp(config(), serverapp))

        url = config().lab_url()
        (runtime_dir() / "lab-url").write_text(url)
        print(f"[ix-mcp] co-edit this notebook live in JupyterLab: {url}", file=sys.stderr, flush=True)

        from .transport import serve

        # The MCP transport is the whole point of the process. Without this
        # callback a crash in the task would be swallowed until GC, leaving the
        # Jupyter Server running while the MCP client hangs on a dead pipe; the
        # callback turns a crash into a loud exit instead. A *normal* return means
        # the client disconnected: we do NOT force-exit there, so the Jupyter
        # Server can flush the collaborative document to its .ipynb before the
        # parent reaps this subprocess (force-exiting on disconnect truncated the
        # last edits before autosave ran).
        self._ix_mcp_task = asyncio.create_task(serve())
        self._ix_mcp_task.add_done_callback(_exit_on_crash)


def _exit_on_crash(task: asyncio.Task) -> None:
    if task.cancelled():
        return
    exc = task.exception()
    if exc is not None:
        print(f"[ix-mcp] MCP transport crashed: {exc!r}", file=sys.stderr, flush=True)
        os._exit(1)
