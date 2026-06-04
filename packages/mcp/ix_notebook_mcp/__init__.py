"""ix-mcp: a notebook-first MCP server.

The agent and a human drive ONE live Jupyter notebook together. Every MCP tool
call edits a real ``.ipynb`` through the Jupyter real-time-collaboration (Yjs)
layer, so a person who opens the same notebook in JupyterLab sees the agent's
cells and outputs appear live, and the work is left behind as a notebook with
outputs that anyone can reopen.

The pieces:
  - :mod:`ix_notebook_mcp.runtime` holds the process-global config (workspace
    dir, bind address, the real stdout the MCP protocol owns).
  - :mod:`ix_notebook_mcp.notebook` reaches the live ``YNotebook`` for a path and
    edits cells inside it (the co-edit boundary).
  - :mod:`ix_notebook_mcp.kernel` runs code on the notebook's own kernel and
    turns kernel messages into notebook outputs.
  - :mod:`ix_notebook_mcp.tools` is the MCP tool surface.
  - :mod:`ix_notebook_mcp.extension` loads all of the above inside a Jupyter
    Server so the MCP code shares the process with the YDoc rooms (the only way
    to co-edit without desyncing the browser).
  - :mod:`ix_notebook_mcp.cli` is the ``ix-mcp`` entrypoint.
"""

from __future__ import annotations

__version__ = "0.1.0"


def _jupyter_server_extension_points() -> list[dict]:
    """Jupyter looks for this on the *package* it is told to load (``ix_notebook_mcp``),
    so it must live here in ``__init__``, not in :mod:`ix_notebook_mcp.extension`.
    The import is deferred so the lightweight CLI paths (``eval``/``exec``) do not
    pull in jupyter_server."""
    from .extension import IxNotebookMCPExtension

    return [{"module": "ix_notebook_mcp.extension", "app": IxNotebookMCPExtension}]
