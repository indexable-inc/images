"""The agent's presentation as structured data: the single source of truth that
both the read-only dashboard and any embedder (the room server) consume.

The dashboard (``dashboard.py``) is one view of this feed, served over HTTP. The
room server is another: it spawns ``ix-mcp`` as the agent's only tool, then
renders the same feed inline in the chat so a turn's tables, plots, and HTML show
up beside the agent's text. Both read the feed the same way -- in-process by
importing this module, or over HTTP via the ``/api/...`` routes ``dashboard.py``
wraps around it -- so the shape is defined once, here.

Three kinds, mirroring the store tables (``store.py``) and ``site/src/lib/types.ts``:

- ``jobs``: every ``python_exec`` run (running or finished), newest first with
  running pinned. Each carries its rich ``outputs`` as nbformat-style mime
  bundles: ``text/html`` is the human view, ``application/x-ix-llm+json`` is what
  the model received. An embedder joins one job by its id (the id a
  ``python_exec`` tool result already names) to render that run's outputs inline.
- ``cells``: the agent's curated highlight reel (``cells.add(...)`` in the
  kernel), in display order. The presentation pane a chat surfaces as artifacts.
- ``resources``: live, self-updating views (a running terminal, a custom widget),
  re-rendered to HTML for as long as their source stays alive.
"""

from __future__ import annotations

import sqlite3

from . import store

# How many recent executions a snapshot carries. Matches the dashboard's prior
# inline limit; running jobs are pinned first by `store.recent`, so an active run
# is never dropped past it.
JOBS_LIMIT = 200


def snapshot(conn: sqlite3.Connection, *, jobs_limit: int = JOBS_LIMIT) -> dict:
    """The whole presentation in one read: ``{"jobs", "cells", "resources"}``.

    The embed contract. A consumer polls this (or, in-process, calls it) to get
    the agent's full current presentation; ``rev`` lets it skip rendering when
    nothing changed (a cheap-to-compute change marker, not a content hash)."""
    jobs = store.recent(conn, limit=jobs_limit)
    cells = store.cells(conn)
    resources = store.live_resources(conn)
    return {
        "jobs": jobs,
        "cells": cells,
        "resources": resources,
        "rev": _rev(jobs, cells, resources),
    }


def job(conn: sqlite3.Connection, job_id: str) -> dict | None:
    """One execution by id, or ``None`` -- the rich outputs for the
    ``jobs['<id>']`` a ``python_exec`` tool result names, so an embedder renders
    that run's tables/plots/HTML beside the tool call that produced them."""
    return store.get(conn, job_id)


def _rev(jobs: list[dict], cells: list[dict], resources: list[dict]) -> str:
    """A change marker for the snapshot: a consumer that polls re-renders only
    when this differs. Built from each row's id and last-write time (``ended_at``
    / ``updated_at``) plus a running job's live ``output`` length, so an in-flight
    job streaming output advances the marker without hashing whole payloads."""
    parts: list[str] = []
    for j in jobs:
        parts.append(
            f"j{j['id']}:{j.get('status')}:{j.get('ended_at') or len(j.get('output') or '')}"
            f":{j.get('line')}"
        )
    for c in cells:
        parts.append(f"c{c['id']}:{c.get('updated_at')}")
    for r in resources:
        parts.append(f"r{r['id']}:{r.get('updated_at')}:{r.get('status')}")
    return str(hash(tuple(parts)))
