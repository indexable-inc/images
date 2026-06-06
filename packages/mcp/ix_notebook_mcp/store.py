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
from pathlib import Path

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
    outputs     TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS executions_started ON executions (started_at);

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
    return conn


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
) -> None:
    conn.execute(
        "UPDATE executions SET status = ?, ended_at = ?, output = ?, result = ?, error = ?, outputs = ? "
        "WHERE id = ?",
        (status, ended_at, output, result, error, json.dumps(outputs or []), id),
    )


def recent(conn: sqlite3.Connection, limit: int = 100) -> list[dict]:
    """The most recent executions, newest first, as plain dicts for the dashboard."""
    conn.row_factory = sqlite3.Row
    # Running jobs sort first so a long-running job is never dropped by the limit
    # (a finished-jobs backlog could otherwise push it past LIMIT); then newest.
    rows = conn.execute(
        "SELECT id, name, code, status, started_at, ended_at, output, result, error, outputs "
        "FROM executions ORDER BY (status = 'running') DESC, started_at DESC LIMIT ?",
        (limit,),
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
