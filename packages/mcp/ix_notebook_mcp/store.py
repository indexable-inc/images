"""The durable execution log: one append-only SQLite database of every
``python_exec`` run plus its captured output, written by the kernel-side runtime
and read by the dashboard.

This is the single source of truth (RFC: write a fact once, derive each view).
The kernel process owns the writes (a job appends its output tail as it runs and
its final status when it ends); the dashboard process only reads. Both open the
same file by the ``IX_MCP_STORE`` path, so the two never share Python objects,
only rows. WAL mode lets the reader see in-flight writes without blocking the
writer.
"""

from __future__ import annotations

import json
import sqlite3
import time
from pathlib import Path


def _now() -> float:
    return time.time()

_SCHEMA = """
CREATE TABLE IF NOT EXISTS executions (
    id          TEXT PRIMARY KEY,
    name        TEXT,
    code        TEXT NOT NULL,
    status      TEXT NOT NULL,
    started_at  REAL NOT NULL,
    ended_at    REAL,
    budget      REAL NOT NULL DEFAULT 15,
    output      TEXT NOT NULL DEFAULT '',
    result      TEXT,
    error       TEXT,
    line        INTEGER,
    error_line  INTEGER,
    outputs     TEXT NOT NULL DEFAULT '[]',
    bindings    TEXT NOT NULL DEFAULT '{}',
    kind        TEXT NOT NULL DEFAULT 'cell',
    namespace   TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS executions_started ON executions (started_at);

-- Session checkpoints: the kernel's user namespace, serialized (dill) after
-- executions finish, so reopening this file restores state instantly instead of
-- re-running every cell. Only the newest row is kept (save_snapshot prunes), so
-- a long session never grows the file by stale checkpoints.
CREATE TABLE IF NOT EXISTS snapshots (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at  REAL NOT NULL,
    blob        BLOB NOT NULL,
    names       TEXT NOT NULL DEFAULT '[]',
    skipped     TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS cells (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL DEFAULT '',
    position    INTEGER NOT NULL,
    outputs     TEXT NOT NULL DEFAULT '[]',
    updated_at  REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS resources (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    kind        TEXT NOT NULL DEFAULT 'html',
    html        TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'live',
    created_at  REAL NOT NULL,
    updated_at  REAL NOT NULL
);
"""


def connect(path: str | Path) -> sqlite3.Connection:
    """Open (creating if needed) the store. WAL so a reader never blocks the
    writer and sees committed in-flight rows; ``busy_timeout`` so the rare
    writer/writer overlap waits rather than raising ``database is locked``."""
    conn = sqlite3.connect(str(path), timeout=5.0, isolation_level=None)
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA busy_timeout=5000")
    conn.executescript(_SCHEMA)
    _migrate(conn)
    return conn


def _migrate(conn: sqlite3.Connection) -> None:
    """Add columns introduced after a store may have first been created. ``CREATE
    TABLE IF NOT EXISTS`` never alters an existing table, so a store written by an
    older build is missing newer columns; add each idempotently."""
    have = {row[1] for row in conn.execute("PRAGMA table_info(executions)")}
    # The kernel and the dashboard open the store from two processes at startup,
    # so both can see a column missing and race to add it; a duplicate-column
    # error means the other won, which is fine. This is a logical error, not
    # SQLITE_BUSY, so busy_timeout does not cover it.
    if "bindings" not in have:
        try:
            conn.execute("ALTER TABLE executions ADD COLUMN bindings TEXT NOT NULL DEFAULT '{}'")
        except sqlite3.OperationalError:
            pass
    if "budget" not in have:
        try:
            conn.execute("ALTER TABLE executions ADD COLUMN budget REAL NOT NULL DEFAULT 15")
        except sqlite3.OperationalError:
            pass
    for column in ("line", "error_line"):
        if column not in have:
            try:
                conn.execute(f"ALTER TABLE executions ADD COLUMN {column} INTEGER")
            except sqlite3.OperationalError:
                pass
    if "kind" not in have:
        try:
            conn.execute("ALTER TABLE executions ADD COLUMN kind TEXT NOT NULL DEFAULT 'cell'")
        except sqlite3.OperationalError:
            pass
    if "namespace" not in have:
        try:
            conn.execute("ALTER TABLE executions ADD COLUMN namespace TEXT NOT NULL DEFAULT '[]'")
        except sqlite3.OperationalError:
            pass


def start(
    conn: sqlite3.Connection,
    *,
    id: str,
    name: str,
    code: str,
    started_at: float,
    budget: float = 15.0,
    kind: str = "cell",
) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO executions (id, name, code, status, started_at, budget, output, kind) "
        "VALUES (?, ?, ?, 'running', ?, ?, '', ?)",
        (id, name, code, started_at, budget, kind),
    )


def rename(conn: sqlite3.Connection, *, id: str, name: str) -> None:
    """Update the display name for an execution already in the store."""
    conn.execute("UPDATE executions SET name = ? WHERE id = ?", (name, id))


def update_output(
    conn: sqlite3.Connection,
    id: str,
    output: str,
    outputs: list | None = None,
    *,
    line: int | None = None,
) -> None:
    """Persist a running job's live output and the cell ``line`` it is executing
    right now (the dashboard's live line highlight; None clears it). When
    ``outputs`` is given (rich display bundles captured so far), update that
    column too so the dashboard can show a long job's in-progress tables/images,
    not only its text."""
    if outputs is None:
        conn.execute(
            "UPDATE executions SET output = ?, line = ? WHERE id = ?", (output, line, id)
        )
    else:
        conn.execute(
            "UPDATE executions SET output = ?, line = ?, outputs = ? WHERE id = ?",
            (output, line, json.dumps(outputs), id),
        )


def finish(
    conn: sqlite3.Connection,
    *,
    id: str,
    status: str,
    ended_at: float,
    output: str,
    result: str | None,
    error: str | None,
    error_line: int | None = None,
    outputs: list | None = None,
    bindings: dict | None = None,
    namespace: list | None = None,
) -> None:
    # `line` (the live executing line) is cleared: a finished job has no current
    # line, only -- when it failed -- the `error_line` it failed on. `namespace`
    # is the kernel's user globals as of this finish; the newest one is the live
    # namespace the dashboard's namespace pane shows.
    conn.execute(
        "UPDATE executions SET status = ?, ended_at = ?, output = ?, result = ?, error = ?, "
        "error_line = ?, line = NULL, outputs = ?, bindings = ?, namespace = ? WHERE id = ?",
        (
            status,
            ended_at,
            output,
            result,
            error,
            error_line,
            json.dumps(outputs or []),
            json.dumps(bindings or {}),
            json.dumps(namespace or []),
            id,
        ),
    )


# The execution columns every reader projects, in one place so `recent` and
# `get` return the identical shape (the embed contract in feed.py depends on it).
_EXEC_COLUMNS = (
    "id, name, code, status, started_at, ended_at, budget, output, result, error, "
    "line, error_line, outputs, bindings, kind"
)


def _exec_row(row: sqlite3.Row) -> dict:
    """One execution row as a plain dict with its JSON columns decoded."""
    d = dict(row)
    d["outputs"] = json.loads(d.get("outputs") or "[]")
    d["bindings"] = json.loads(d.get("bindings") or "{}")
    return d


def recent(conn: sqlite3.Connection, limit: int = 100) -> list[dict]:
    """The most recent executions, newest first, as plain dicts for the dashboard."""
    conn.row_factory = sqlite3.Row
    # Running jobs sort first so a long-running job is never dropped by the limit
    # (a finished-jobs backlog could otherwise push it past LIMIT); then newest.
    rows = conn.execute(
        f"SELECT {_EXEC_COLUMNS} "
        "FROM executions ORDER BY (status = 'running') DESC, started_at DESC LIMIT ?",
        (limit,),
    ).fetchall()
    return [_exec_row(r) for r in rows]


def latest_namespace(conn: sqlite3.Connection) -> list[dict]:
    """The kernel's user globals as of the most recently *finished* run — the live
    namespace the dashboard's namespace pane shows.

    Reads the newest execution with an ``ended_at`` (a running job has not written
    its namespace yet, so it is excluded), regardless of whether that namespace is
    empty. Reading the newest finished run rather than the newest *non-empty* one
    is what keeps the pane honest: after a run clears the namespace (a reset, or
    `del`-ing the last variable) the latest finished run records ``[]`` and the
    pane drops, instead of pinning the last non-empty snapshot as stale data.
    Empty before any run finishes."""
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        "SELECT namespace FROM executions "
        "WHERE ended_at IS NOT NULL "
        "ORDER BY ended_at DESC LIMIT 1"
    ).fetchone()
    if row is None:
        return []
    try:
        return json.loads(row["namespace"] or "[]")
    except (ValueError, TypeError):
        return []


def get(conn: sqlite3.Connection, id: str) -> dict | None:
    """One execution by id (or None), same shape as a `recent` row. An embedder
    joins this to the ``jobs['<id>']`` a ``python_exec`` tool result already names,
    to render that run's rich outputs inline with the tool call."""
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        f"SELECT {_EXEC_COLUMNS} FROM executions WHERE id = ?", (id,)
    ).fetchone()
    return _exec_row(row) if row is not None else None


# --------------------------------------------------------------------------- #
# Cells: the agent's curated presentation pane.
#
# Unlike executions (append-only history) and resources (live, self-updating
# views), cells are whatever the agent chooses to PRESENT: an ordered highlight
# reel it rebuilds as the session evolves. The kernel mirrors the whole set on
# change (it is small and order matters), so the store always holds the current
# presentation and the dashboard just lists it.
# --------------------------------------------------------------------------- #


def replace_cells(conn: sqlite3.Connection, items: list[dict]) -> None:
    """Replace the presentation with ``items`` (each ``{id, title, position,
    outputs}``) in one transaction, so a reader never sees a half-written set."""
    rows = [
        (it["id"], it.get("title", ""), it["position"], json.dumps(it.get("outputs") or []), _now())
        for it in items
    ]
    conn.execute("BEGIN IMMEDIATE")
    try:
        conn.execute("DELETE FROM cells")
        if rows:
            conn.executemany(
                "INSERT INTO cells (id, title, position, outputs, updated_at) VALUES (?, ?, ?, ?, ?)",
                rows,
            )
        conn.execute("COMMIT")
    except Exception:
        conn.execute("ROLLBACK")
        raise


def cells(conn: sqlite3.Connection) -> list[dict]:
    """The current presentation, in display order, as plain dicts."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT id, title, position, outputs, updated_at FROM cells ORDER BY position ASC"
    ).fetchall()
    out = []
    for r in rows:
        d = dict(r)
        d["outputs"] = json.loads(d.get("outputs") or "[]")
        out.append(d)
    return out


# --------------------------------------------------------------------------- #
# Resources: live, self-updating views (a Tui's screen, a custom HTML widget).
#
# A resource is the long-lived counterpart to an execution: an execution is a
# finished fact (one row, written once and amended to 'done'), while a resource
# is a *living* thing the kernel keeps re-rendering to HTML for as long as it is
# alive. The kernel upserts the latest HTML each flush tick and flips status to
# 'closed' when the underlying object goes away; the dashboard sidebar reads the
# still-live rows. Same single-writer / many-reader split as executions.
# --------------------------------------------------------------------------- #


def upsert_resource(
    conn: sqlite3.Connection,
    *,
    id: str,
    title: str,
    kind: str,
    html: str,
    status: str,
    created_at: float,
    updated_at: float,
) -> None:
    """Insert a resource or refresh its rendered HTML/status in place."""
    conn.execute(
        "INSERT INTO resources (id, title, kind, html, status, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET "
        "title = excluded.title, kind = excluded.kind, html = excluded.html, "
        "status = excluded.status, updated_at = excluded.updated_at",
        (id, title, kind, html, status, created_at, updated_at),
    )


def close_resource(conn: sqlite3.Connection, *, id: str, updated_at: float) -> None:
    """Mark a resource closed so the sidebar drops it (the row stays for history)."""
    conn.execute(
        "UPDATE resources SET status = 'closed', updated_at = ? WHERE id = ?",
        (updated_at, id),
    )


# --------------------------------------------------------------------------- #
# Sessions: the pieces that make one store file a reopenable notebook.
#
# A session file carries three things: the execution log (the cells and their
# outputs, already above), the latest namespace snapshot (instant state on
# reopen), and the bookkeeping to catch up the gap -- cells that finished after
# the snapshot was taken are re-run on open, ordered by start time.
# --------------------------------------------------------------------------- #


def save_snapshot(
    conn: sqlite3.Connection,
    *,
    created_at: float,
    blob: bytes,
    names: list[str],
    skipped: list[dict],
) -> None:
    """Persist a namespace checkpoint and prune older ones in the same
    transaction, so the file holds exactly one snapshot and a reader never sees
    zero or two."""
    conn.execute("BEGIN IMMEDIATE")
    try:
        conn.execute(
            "INSERT INTO snapshots (created_at, blob, names, skipped) VALUES (?, ?, ?, ?)",
            (created_at, blob, json.dumps(names), json.dumps(skipped)),
        )
        conn.execute(
            "DELETE FROM snapshots WHERE id != (SELECT MAX(id) FROM snapshots)"
        )
        conn.execute("COMMIT")
    except Exception:
        conn.execute("ROLLBACK")
        raise


def latest_snapshot(conn: sqlite3.Connection) -> dict | None:
    """The newest namespace checkpoint, or None for a fresh session file."""
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        "SELECT created_at, blob, names, skipped FROM snapshots ORDER BY id DESC LIMIT 1"
    ).fetchone()
    if row is None:
        return None
    d = dict(row)
    d["names"] = json.loads(d.get("names") or "[]")
    d["skipped"] = json.loads(d.get("skipped") or "[]")
    return d


def mark_interrupted(conn: sqlite3.Connection, *, ended_at: float) -> int:
    """Close out rows left 'running' by a previous server (it died or was
    killed mid-cell), so a reopened session reads honestly: those cells did not
    finish and their effects are not in any snapshot."""
    cur = conn.execute(
        "UPDATE executions SET status = 'interrupted', ended_at = ?, line = NULL "
        "WHERE status = 'running'",
        (ended_at,),
    )
    # A live resource (a Tui screen, a widget) is a view over an object in the
    # dead kernel; nothing can re-render it, so close it rather than show a
    # frozen pane as live.
    conn.execute(
        "UPDATE resources SET status = 'closed', updated_at = ? WHERE status != 'closed'",
        (ended_at,),
    )
    return cur.rowcount


def replayable(conn: sqlite3.Connection, since: float | None) -> list[dict]:
    """The cells a reopened session re-runs to catch the namespace up: original
    (kind='cell') successful executions that finished after ``since`` (the latest
    snapshot's timestamp; None replays the whole log, the no-snapshot fallback).
    Replay rows themselves are excluded -- their effects are captured by the
    snapshot taken right after a restore, so including them would double-run
    every cell on the next reopen.

    The anchor is ``ended_at``, not ``started_at``: a cell still running when the
    snapshot was written has only partial effects in it, and re-running the cell
    (it finished later, so ``ended_at`` > ``since``) overwrites the partial state
    with the full result."""
    conn.row_factory = sqlite3.Row
    anchor = "" if since is None else " AND ended_at > ?"
    params = () if since is None else (since,)
    rows = conn.execute(
        "SELECT id, name, code FROM executions "
        f"WHERE status = 'done' AND kind = 'cell'{anchor} ORDER BY started_at ASC",
        params,
    ).fetchall()
    return [dict(r) for r in rows]


def live_resources(conn: sqlite3.Connection) -> list[dict]:
    """Every resource not yet closed, oldest first, as plain dicts for the sidebar."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT id, title, kind, html, status, created_at, updated_at "
        "FROM resources WHERE status != 'closed' ORDER BY created_at ASC"
    ).fetchall()
    return [dict(r) for r in rows]
