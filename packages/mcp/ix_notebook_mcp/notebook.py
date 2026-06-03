"""Reach and edit the *live* collaborative notebook.

The single most important function here is :func:`get_ynotebook`: it returns the
exact ``YNotebook`` (a pycrdt-backed CRDT) that a human's open JupyterLab tab is
subscribed to. Editing that object is what makes co-editing work, every change
is broadcast to all connected browsers and persisted to the ``.ipynb`` on disk by
jupyter-collaboration. Writing the file directly instead would raise an
"out-of-band change" desync in the browser, so cell edits always go through the
YNotebook returned here.

The access path mirrors jupyter-collaboration's own internals (and what
datalayer/jupyter-mcp-server settled on in PR #135): the YDoc rooms live on the
``jupyter_server_ydoc`` extension's ``ywebsocket_server``, keyed by a room id
built from the file's stable id. We reach it through ``extension_manager``
because jupyter-collaboration does not register the room manager in
``web_app.settings``.
"""

from __future__ import annotations

import nbformat
from jupyter_ydoc import YNotebook

from .runtime import RUNTIME


def ensure_notebook_file(rel_path: str) -> str:
    """Create an empty valid notebook on disk if it does not exist yet.

    A file must exist before the file-id manager can assign it the stable id the
    YDoc room is keyed on, so notebook creation is "write an empty .ipynb, then
    open its room". An existing file is left untouched.
    """
    if not rel_path.endswith(".ipynb"):
        rel_path = f"{rel_path}.ipynb"
    abspath = RUNTIME.abspath(rel_path)
    if not abspath.exists():
        abspath.parent.mkdir(parents=True, exist_ok=True)
        nbformat.write(nbformat.v4.new_notebook(), abspath)
    return rel_path


async def get_ynotebook(rel_path: str) -> YNotebook:
    """Return the live collaborative ``YNotebook`` for ``rel_path``.

    Uses ``jupyter_server_ydoc``'s public ``get_document`` with ``copy=False`` so
    we get the *actual* room document (not a snapshot): edits propagate to every
    connected browser and are persisted to disk by the collaboration layer.
    ``create=True`` opens the room on the server side even before any browser has
    connected, so the agent can build a notebook and a human can then join it.
    """
    serverapp = RUNTIME.serverapp
    if serverapp is None:
        raise RuntimeError("Jupyter Server is not running; call inside `ix-mcp serve`")

    ydoc_ext = serverapp.extension_manager.extension_points["jupyter_server_ydoc"].app
    ynb = await ydoc_ext.get_document(
        path=rel_path,
        content_type="notebook",
        file_format="json",
        copy=False,
        create=True,
    )
    if ynb is None:
        raise RuntimeError(f"could not open collaborative document for {rel_path!r}")
    return ynb


def cell_index_by_id(ynb: YNotebook, cell_id: str) -> int:
    """Find a cell's current index by its stable id, or raise. Indices shift as
    cells are inserted/deleted, so anything that must survive concurrent edits
    addresses cells by id and re-resolves the index at use time."""
    for index in range(len(ynb.ycells)):
        if ynb.get_cell(index).get("id") == cell_id:
            return index
    raise KeyError(f"no cell with id {cell_id!r}")


def add_cell(ynb: YNotebook, source: str, cell_type: str, index: int) -> dict:
    """Insert a cell and return its serialized form (carrying the assigned id).

    ``index`` of -1 (or past the end) appends. The insert happens in one CRDT
    transaction so collaborators never observe a half-built cell.
    """
    if cell_type == "code":
        cell = nbformat.v4.new_code_cell(source)
    elif cell_type == "markdown":
        cell = nbformat.v4.new_markdown_cell(source)
    elif cell_type == "raw":
        cell = nbformat.v4.new_raw_cell(source)
    else:
        raise ValueError(f"unknown cell_type {cell_type!r}")

    count = len(ynb.ycells)
    with ynb.ydoc.transaction():
        if index < 0 or index >= count:
            ynb.append_cell(cell)
            position = count
        else:
            ynb.ycells.insert(index, ynb.create_ycell(cell))
            position = index
    placed = ynb.get_cell(position)
    placed["_index"] = position
    return placed


def delete_cell(ynb: YNotebook, index: int) -> None:
    with ynb.ydoc.transaction():
        del ynb.ycells[index]


def overwrite_cell_source(ynb: YNotebook, index: int, source: str) -> dict:
    """Replace a cell's source and clear its (now stale) outputs."""
    cell = ynb.get_cell(index)
    cell["source"] = source
    if cell.get("cell_type") == "code":
        cell["outputs"] = []
        cell["execution_count"] = None
    ynb.set_cell(index, cell)
    return ynb.get_cell(index)


def read_cells(ynb: YNotebook) -> list[dict]:
    return [ynb.get_cell(i) | {"_index": i} for i in range(len(ynb.ycells))]
