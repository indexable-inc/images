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

import contextlib
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
    namespace   TEXT NOT NULL DEFAULT '[]',
    topic       TEXT NOT NULL DEFAULT ''
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
    execution_id TEXT NOT NULL DEFAULT '',
    created_at  REAL NOT NULL,
    updated_at  REAL NOT NULL
);

-- This MCP session's identity: one singleton row (id = 0). Every run in this
-- store belongs to one session (one MCP client talking to one `ix-mcp serve`
-- process); the dashboard groups runs by it and lists each session by `name`.
-- `name` is the effective label (the user-set name, else a default derived from
-- the connecting client and the kernel's workdir); `client` is the raw client
-- identity for display. The kernel is the sole writer (see runtime.Session).
CREATE TABLE IF NOT EXISTS session (
    id          INTEGER PRIMARY KEY CHECK (id = 0),
    name        TEXT NOT NULL DEFAULT '',
    client      TEXT NOT NULL DEFAULT '',
    updated_at  REAL NOT NULL
);

-- Input channels: the address registry behind interactive resources. The kernel
-- opens a channel when the agent creates an `Input`/`ask` (the rendered HTML's
-- `ixSubmit` posts to this id), and closes it when the input is done. The
-- dashboard's `/api/input` write path is the only OTHER process that touches this
-- table, and only to READ it: a POST is accepted only for an `open` channel, so a
-- submission for a finished or never-created channel never enters the queue. The
-- id is NOT a secret (it rides in the HTML the read endpoints serve); it is just
-- an address. `/api/input` authorizes by the network boundary (see dashboard.py).
CREATE TABLE IF NOT EXISTS channels (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'open',
    created_at  REAL NOT NULL,
    updated_at  REAL NOT NULL
);

-- Inputs: the user's submissions, an append-only queue from the browser to the
-- kernel. The dashboard's `/api/input` appends one row per submission; the
-- kernel runtime drains them on its flush tick, delivers each to the awaiting
-- `Channel`, and DELETEs it (the row is a transient message, not history), so the
-- table stays empty between submissions. `seq` orders delivery; `channel` joins
-- to `channels`. This is the one place the two processes have a writer each on
-- the same table family (server inserts, kernel deletes); WAL keeps them honest.
CREATE TABLE IF NOT EXISTS inputs (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    channel     TEXT NOT NULL,
    payload     TEXT NOT NULL,
    created_at  REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS inputs_channel ON inputs (channel, seq);

-- Outbox: kernel -> agent push queue behind the Claude Code channel. The kernel's
-- `notify()` appends one row per event; the MCP transport drains them on its poll
-- tick and emits each as a `notifications/claude/channel` MCP notification, so
-- kernel code can wake the connected agent session. Same transient-message
-- discipline as `inputs`: the drain DELETEs what it delivers, so the table stays
-- empty between events. `meta` is a JSON object of identifier-keyed strings (they
-- become attributes on the client's <channel> tag).
CREATE TABLE IF NOT EXISTS outbox (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    content     TEXT NOT NULL,
    meta        TEXT NOT NULL DEFAULT '{}',
    created_at  REAL NOT NULL
);

-- Resource events: the kernel/server -> browser stream behind interactive
-- resources. Action results and errors (kernel) and agent `reply` messages (MCP
-- server) append here; the dashboard's SSE endpoint streams rows to every page
-- subscribed to that resource. Append-only with age-based pruning rather than
-- delete-on-read, because several pages may subscribe to one resource and each
-- reads forward from its own seq.
CREATE TABLE IF NOT EXISTS events (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    resource    TEXT NOT NULL,
    kind        TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS events_resource ON events (resource, seq);
"""


def connect(path: str | Path) -> sqlite3.Connection:
    """Open (creating if needed) the store. WAL so a reader never blocks the
    writer and sees committed in-flight rows; ``busy_timeout`` so the rare
    writer/writer overlap waits rather than raising ``database is locked``."""
    # The store is shared across processes (kernel writes, dashboard reads), so a
    # real file path is required. Guard None explicitly: sqlite3.connect(str(None))
    # would otherwise silently create a database in a file literally named "None"
    # in the cwd instead of failing. See indexable-inc/index#1100.
    if path is None:
        raise ValueError("store path is required (got None)")
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
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE executions ADD COLUMN bindings TEXT NOT NULL DEFAULT '{}'")
    if "budget" not in have:
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE executions ADD COLUMN budget REAL NOT NULL DEFAULT 15")
    for column in ("line", "error_line"):
        if column not in have:
            with contextlib.suppress(sqlite3.OperationalError):
                conn.execute(f"ALTER TABLE executions ADD COLUMN {column} INTEGER")
    if "kind" not in have:
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE executions ADD COLUMN kind TEXT NOT NULL DEFAULT 'cell'")
    if "namespace" not in have:
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE executions ADD COLUMN namespace TEXT NOT NULL DEFAULT '[]'")
    if "topic" not in have:
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE executions ADD COLUMN topic TEXT NOT NULL DEFAULT ''")
    resource_have = {row[1] for row in conn.execute("PRAGMA table_info(resources)")}
    if "execution_id" not in resource_have:
        with contextlib.suppress(sqlite3.OperationalError):
            conn.execute("ALTER TABLE resources ADD COLUMN execution_id TEXT NOT NULL DEFAULT ''")


def start(
    conn: sqlite3.Connection,
    *,
    id: str,
    name: str,
    code: str,
    started_at: float,
    budget: float = 15.0,
    kind: str = "cell",
    topic: str = "",
) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO executions (id, name, code, status, started_at, budget, output, kind, topic) "
        "VALUES (?, ?, ?, 'running', ?, ?, '', ?, ?)",
        (id, name, code, started_at, budget, kind, topic),
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
    "line, error_line, outputs, bindings, kind, topic"
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
        f"SELECT {_EXEC_COLUMNS} "  # noqa: S608 -- _EXEC_COLUMNS is a module-level constant, not user input
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
        f"SELECT {_EXEC_COLUMNS} FROM executions WHERE id = ?", (id,)  # noqa: S608 -- _EXEC_COLUMNS is a module-level constant, not user input
    ).fetchone()
    return _exec_row(row) if row is not None else None


# --------------------------------------------------------------------------- #
# Session: this server's identity, grouping every run in the store.
# --------------------------------------------------------------------------- #


def set_session(conn: sqlite3.Connection, *, name: str, client: str) -> None:
    """Write this session's effective label and client identity (the singleton
    id-0 row). The kernel runtime is the sole writer; the dashboard reads it to
    label the session in its selector."""
    conn.execute(
        "INSERT INTO session (id, name, client, updated_at) VALUES (0, ?, ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET "
        "name = excluded.name, client = excluded.client, updated_at = excluded.updated_at",
        (name, client, _now()),
    )


def get_session(conn: sqlite3.Connection) -> dict | None:
    """This session's ``{name, client, updated_at}``, or None before it is set."""
    conn.row_factory = sqlite3.Row
    row = conn.execute(
        "SELECT name, client, updated_at FROM session WHERE id = 0"
    ).fetchone()
    return dict(row) if row is not None else None


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
    execution_id: str = "",
) -> None:
    """Insert a resource or refresh its rendered HTML/status in place."""
    conn.execute(
        "INSERT INTO resources (id, title, kind, html, status, execution_id, created_at, updated_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET "
        "title = excluded.title, kind = excluded.kind, html = excluded.html, "
        "status = excluded.status, execution_id = excluded.execution_id, updated_at = excluded.updated_at",
        (id, title, kind, html, status, execution_id, created_at, updated_at),
    )


def close_resource(conn: sqlite3.Connection, *, id: str, updated_at: float) -> None:
    """Mark a resource closed while keeping its final pane visible."""
    conn.execute(
        "UPDATE resources SET status = 'closed', updated_at = ? WHERE id = ?",
        (updated_at, id),
    )


# --------------------------------------------------------------------------- #
# Channels + inputs: the browser -> kernel write path behind interactive
# resources. The kernel owns the `channels` lifecycle (open on create, closed on
# done); the dashboard's `/api/input` reads `channels` to authorize a submission
# and appends to `inputs`; the kernel drains `inputs` on its flush tick. See the
# schema comments for the trust model (the channel id is a bearer capability).
# --------------------------------------------------------------------------- #


def open_channel(conn: sqlite3.Connection, *, id: str, title: str) -> None:
    """Open (or re-open) an input channel so `/api/input` accepts submissions for
    it. Idempotent: re-opening a known id refreshes its title and `open` status."""
    now = _now()
    conn.execute(
        "INSERT INTO channels (id, title, status, created_at, updated_at) "
        "VALUES (?, ?, 'open', ?, ?) "
        "ON CONFLICT(id) DO UPDATE SET title = excluded.title, status = 'open', "
        "updated_at = excluded.updated_at",
        (id, title, now, now),
    )


def close_channel(conn: sqlite3.Connection, *, id: str) -> None:
    """Close a channel so `/api/input` rejects further submissions, and drop any
    of its still-undelivered inputs (no awaiter remains to read them)."""
    conn.execute(
        "UPDATE channels SET status = 'closed', updated_at = ? WHERE id = ?", (_now(), id)
    )
    conn.execute("DELETE FROM inputs WHERE channel = ?", (id,))


def channel_open(conn: sqlite3.Connection, id: str) -> bool:
    """Whether ``id`` names a channel currently accepting input. The dashboard's
    `/api/input` gate: an unknown or closed id is refused (so a submission for a
    finished or never-created channel never enters the queue)."""
    row = conn.execute("SELECT status FROM channels WHERE id = ?", (id,)).fetchone()
    return row is not None and row[0] == "open"


def add_input(conn: sqlite3.Connection, *, channel: str, payload: str) -> None:
    """Append one user submission (a JSON ``payload`` string) for ``channel``. The
    dashboard is the sole inserter; the kernel drains and deletes."""
    conn.execute(
        "INSERT INTO inputs (channel, payload, created_at) VALUES (?, ?, ?)",
        (channel, payload, _now()),
    )


def pending_inputs(conn: sqlite3.Connection) -> list[dict]:
    """Every queued submission, oldest first, as ``{seq, channel, payload}`` dicts.
    The kernel reads these on its flush tick, delivers each to the matching live
    channel, and calls :func:`delete_inputs` with the seqs it handled."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT seq, channel, payload FROM inputs ORDER BY seq ASC"
    ).fetchall()
    return [dict(r) for r in rows]


def delete_inputs(conn: sqlite3.Connection, seqs: list[int]) -> None:
    """Remove the input rows the kernel just delivered (by ``seq``), so each
    submission is delivered exactly once and the queue stays empty between them."""
    if not seqs:
        return
    placeholders = ",".join("?" for _ in seqs)
    conn.execute(f"DELETE FROM inputs WHERE seq IN ({placeholders})", seqs)  # noqa: S608 -- placeholders are bound params, seqs are the values


# --------------------------------------------------------------------------- #
# Outbox + events: the kernel -> agent and kernel/server -> browser push paths
# behind the Claude Code channel and interactive resource actions. The kernel
# writes `outbox` (notify()) and `events` (action results); the MCP transport
# drains `outbox` into channel notifications; the MCP `reply` tool writes
# `events`; the dashboard's SSE endpoint reads `events`.
# --------------------------------------------------------------------------- #

# Events older than this are pruned on write. The stream is a live feed, not
# history: a page subscribes and reads forward, so a row only matters for as long
# as a subscriber might still be catching up on a slow poll tick.
_EVENT_MAX_AGE_SECONDS = 3600.0


def add_outbox(conn: sqlite3.Connection, *, content: str, meta: str) -> None:
    """Queue one channel event (``meta`` is a JSON object of identifier-keyed
    strings). The kernel is the sole inserter; the MCP transport drains."""
    conn.execute(
        "INSERT INTO outbox (content, meta, created_at) VALUES (?, ?, ?)",
        (content, meta, _now()),
    )


def take_outbox(conn: sqlite3.Connection) -> list[dict]:
    """Every queued channel event, oldest first, consuming the rows. Like
    ``_drain_inputs``: DELETE before delivering, so an event can never be emitted
    twice; a crash between the delete and the send loses at most one batch."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT seq, content, meta FROM outbox ORDER BY seq ASC"
    ).fetchall()
    if not rows:
        return []
    placeholders = ",".join("?" for _ in rows)
    conn.execute(
        f"DELETE FROM outbox WHERE seq IN ({placeholders})",  # noqa: S608 -- placeholders are bound params
        [r["seq"] for r in rows],
    )
    return [dict(r) for r in rows]


def add_event(conn: sqlite3.Connection, *, resource: str, kind: str, body: str) -> None:
    """Append one resource event (``body`` is a JSON object) and prune rows past
    the age cap, so the live feed never grows the store without bound."""
    now = _now()
    conn.execute(
        "INSERT INTO events (resource, kind, body, created_at) VALUES (?, ?, ?, ?)",
        (resource, kind, body, now),
    )
    conn.execute(
        "DELETE FROM events WHERE created_at < ?", (now - _EVENT_MAX_AGE_SECONDS,)
    )


def latest_event_seq(conn: sqlite3.Connection, resource: str) -> int:
    """The newest event seq for ``resource`` (0 when none): where a fresh SSE
    subscriber starts, so it streams new events only, never replayed history."""
    row = conn.execute(
        "SELECT MAX(seq) FROM events WHERE resource = ?", (resource,)
    ).fetchone()
    return int(row[0]) if row and row[0] is not None else 0


def events_after(conn: sqlite3.Connection, resource: str, seq: int) -> list[dict]:
    """The events for ``resource`` with seq > ``seq``, oldest first."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT seq, kind, body, created_at FROM events "
        "WHERE resource = ? AND seq > ? ORDER BY seq ASC",
        (resource, seq),
    ).fetchall()
    return [dict(r) for r in rows]


def resource_live(conn: sqlite3.Connection, id: str) -> bool:
    """Whether ``id`` names a resource not yet closed: the `reply` tool's gate, so
    a reply to a finished or mistyped resource fails loudly instead of streaming
    into a feed no page reads."""
    row = conn.execute("SELECT status FROM resources WHERE id = ?", (id,)).fetchone()
    return row is not None and row[0] != "closed"


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
    # An open input channel awaits a submission in a coroutine that died with the
    # kernel; close it so a stale capability cannot accept input nobody reads, and
    # drop any queued-but-undelivered submissions for the same reason.
    conn.execute(
        "UPDATE channels SET status = 'closed', updated_at = ? WHERE status != 'closed'",
        (ended_at,),
    )
    conn.execute("DELETE FROM inputs")
    # A dead kernel's undelivered channel pushes and its resources' event feed
    # describe state that no longer exists; a reopened session must not fire
    # stale notifications into a fresh agent session or replay them to a page.
    conn.execute("DELETE FROM outbox")
    conn.execute("DELETE FROM events")
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
        "SELECT id, name, code FROM executions "  # noqa: S608 -- anchor is one of "" or " AND ended_at > ?", not user input
        f"WHERE status = 'done' AND kind = 'cell'{anchor} ORDER BY started_at ASC",
        params,
    ).fetchall()
    return [dict(r) for r in rows]


def live_resources(conn: sqlite3.Connection) -> list[dict]:
    """Every resource, oldest first, as plain dicts for the sidebar."""
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT id, title, kind, html, status, execution_id, created_at, updated_at "
        "FROM resources ORDER BY created_at ASC"
    ).fetchall()
    return [dict(r) for r in rows]
