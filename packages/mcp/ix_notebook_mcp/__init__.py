"""ix-mcp: a single-tool Python execution MCP server.

One tool, ``python_exec``, runs code on ONE shared, persistent IPython kernel.
Every execution runs as an asyncio task on that kernel's event loop, so many run
concurrently and none blocks the others. A call waits up to ``budget`` seconds;
if the work is still going it keeps running in the background, registered in the
in-kernel ``jobs`` dict that later ``python_exec`` calls inspect, await, or
cancel. Each run is logged to a SQLite store that an auto-started dashboard
renders, so a human can watch every running thing and its output live.

The pieces:
  - :mod:`ix_notebook_mcp.runtime` is the in-kernel runtime (``jobs``/``Job``/
    ``__ix_exec``) loaded by the shipped IPython startup script.
  - :mod:`ix_notebook_mcp.store` is the append-only SQLite execution log.
  - :mod:`ix_notebook_mcp.kernel` owns the one kernel and drives executions.
  - :mod:`ix_notebook_mcp.outputs` renders kernel messages for the agent.
  - :mod:`ix_notebook_mcp.dashboard` serves the live view of the store.
  - :mod:`ix_notebook_mcp.tools` is the MCP tool surface.
  - :mod:`ix_notebook_mcp.cli` is the ``ix-mcp`` entrypoint.
"""

from __future__ import annotations

__version__ = "0.2.0"
