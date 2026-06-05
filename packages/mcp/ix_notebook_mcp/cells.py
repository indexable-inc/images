"""Pure cell transforms over a live ``YNotebook``.

These functions are stateless: they take the collaborative notebook and edit its
cells inside CRDT transactions, so a collaborator never observes a half-built
cell. They hold no server state, which keeps the editing logic testable in
isolation from the kernel and the MCP layer.
"""

from __future__ import annotations

import nbformat
from jupyter_ydoc import YNotebook

_NEW_CELL = {
    "code": nbformat.v4.new_code_cell,
    "markdown": nbformat.v4.new_markdown_cell,
    "raw": nbformat.v4.new_raw_cell,
}


def add(ynb: YNotebook, source: str, cell_type: str, index: int) -> dict:
    """Insert a cell and return its serialized form (carrying the assigned id).
    ``index`` of -1 (or past the end) appends."""
    make = _NEW_CELL.get(cell_type)
    if make is None:
        raise ValueError(f"unknown cell_type {cell_type!r}")
    cell = make(source)
    count = len(ynb.ycells)
    with ynb.ydoc.transaction():
        if index < 0 or index >= count:
            ynb.append_cell(cell)
            position = count
        else:
            ynb.ycells.insert(index, ynb.create_ycell(cell))
            position = index
    return read_one(ynb, position)


def delete(ynb: YNotebook, index: int) -> None:
    with ynb.ydoc.transaction():
        del ynb.ycells[index]


def overwrite_source(ynb: YNotebook, index: int, source: str) -> dict:
    """Replace a cell's source and clear its now-stale outputs."""
    cell = ynb.get_cell(index)
    cell["source"] = source
    if cell.get("cell_type") == "code":
        cell["outputs"] = []
        cell["execution_count"] = None
    ynb.set_cell(index, cell)
    return read_one(ynb, index)


def set_outputs(ynb: YNotebook, index: int, outputs: list[dict], execution_count: int | None) -> None:
    """Write execution results into a cell so collaborators see them and they
    persist to the ``.ipynb``."""
    cell = ynb.get_cell(index)
    cell["outputs"] = outputs
    cell["execution_count"] = execution_count
    ynb.set_cell(index, cell)


def index_of(ynb: YNotebook, cell_id: str) -> int:
    """Find a cell's current index by stable id, or raise. Indices shift as cells
    are inserted/deleted, so anything that must survive concurrent edits addresses
    cells by id and re-resolves the index at use time."""
    for index in range(len(ynb.ycells)):
        if ynb.get_cell(index).get("id") == cell_id:
            return index
    raise KeyError(f"no cell with id {cell_id!r}")


def read_one(ynb: YNotebook, index: int) -> dict:
    return ynb.get_cell(index) | {"index": index}


def read_all(ynb: YNotebook) -> list[dict]:
    return [read_one(ynb, i) for i in range(len(ynb.ycells))]
