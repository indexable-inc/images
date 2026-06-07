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
    output      TEXT NOT NULL DEFAULT '',
    result      TEXT,
    error       TEXT,
    outputs     TEXT NOT NULL DEFAULT '[]',
    bindings    TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS executions_started ON executions (started_at);

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
    if "bindings" not in have:
        try:
            conn.execute("ALTER TABLE executions ADD COLUMN bindings TEXT NOT NULL DEFAULT '{}'")
        except sqlite3.OperationalError:
            # The kernel and the dashboard open the store from two processes at
            # startup, so both can see the column missing and race to add it; a
            # duplicate-column error here means the other won, which is fine. This
            # is a logical error, not SQLITE_BUSY, so busy_timeout does not cover it.
            pass


def start(conn: sqlite3.Connection, *, id: str, name: str, code: str, started_at: float) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO executions (id, name, code, status, started_at, output) "
        "VALUES (?, ?, ?, 'running', ?, '')",
        (id, name, code, started_at),
    )


def update_output(conn: sqlite3.Connection, id: str, output: str, outputs: list | None = None) -> None:
    """Persist a running job's live output. When ``outputs`` is given (rich display
    bundles captured so far), update that column too so the dashboard can show a
    long job's in-progress tables/images, not only its text."""
    if outputs is None:
        conn.execute("UPDATE executions SET output = ? WHERE id = ?", (output, id))
    else:
        conn.execute(
            "UPDATE executions SET output = ?, outputs = ? WHERE id = ?",
            (output, json.dumps(outputs), id),
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
    outputs: list | None = None,
    bindings: dict | None = None,
) -> None:
    conn.execute(
        "UPDATE executions SET status = ?, ended_at = ?, output = ?, result = ?, error = ?, "
        "outputs = ?, bindings = ? WHERE id = ?",
        (status, ended_at, output, result, error, json.dumps(outputs or []), json.dumps(bindings or {}), id),
    )


def recent(conn: sqlite3.Connection, limit: int = 100) -> list[dict]:
    """The most recent executions, newest first, as plain dicts for the dashboard."""
    conn.row_factory = sqlite3.Row
    # Running jobs sort first so a long-running job is never dropped by the limit
    # (a finished-jobs backlog could otherwise push it past LIMIT); then newest.
    rows = conn.execute(
        "SELECT id, name, code, status, started_at, ended_at, output, result, error, outputs, bindings "
        "FROM executions ORDER BY (status = 'running') DESC, started_at DESC LIMIT ?",
        (limit,),
    ).fetchall()
    out = []
    for r in rows:
        d = dict(r)
        d["outputs"] = json.loads(d.get("outputs") or "[]")
        d["bindings"] = json.loads(d.get("bindings") or "{}")
        out.append(d)
    return out


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


def live_resources(conn: sqlite3.Connection) -> list[dict]:
    """Every resource not yet closed, oldest first, as plain dicts for the sidebar."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT id, title, kind, html, status, created_at, updated_at "
        "FROM resources WHERE status != 'closed' ORDER BY created_at ASC"
    ).fetchall()
    return [dict(r) for r in rows]
