"""Bridge the execution store into the Loro dashboard hub.

The MCP is a *producer*, not a dashboard owner: this reads the same SQLite store
the read-only API serves and republishes it as dashboard-core panes — one
``exec`` pane per run, one ``html`` pane per live resource, and one ``data`` pane
(the ``namespace`` renderer) for the kernel's live globals — over the producer
socket. The standalone ``dashboard`` hub then renders the MCP's activity in one
canvas alongside every other producer (the TUI's terminals, a VM's screen),
which is the convergence: one Loro interface, many producers.

It polls the store on a short interval and publishes only when the pane set
changes, so an idle session is silent. Best-effort throughout: if the producer
socket cannot bind, the MCP keeps working with no dashboard panes.
"""

from __future__ import annotations

import asyncio
import html
import json
import sqlite3
from pathlib import Path

from . import store
from .outputs import IX_VIEW_MIME
from .produce import PaneProducer, data_pane, exec_pane, html_pane

# How often to resample the store. Local SQLite in WAL mode, so a poll is cheap;
# fast enough that a run appears promptly, slow enough to stay idle-quiet.
_POLL_SECONDS = 0.25

# Match the read-only API's window so the board and the API agree on which runs
# are "recent".
_JOBS_LIMIT = 100


def _ok(status: str) -> bool | None:
    """An exec pane's ``ok``: None while running, True on a clean finish, False on
    an error/interrupt/cancel — so the card's LED reads run → done/failed."""
    if status == "running":
        return None
    return status == "done"


def _output_html(out: dict) -> str:
    """One captured rich output (an nbformat-style mime bundle) as HTML for the
    hub's sandboxed frame: an image, else producer HTML/SVG, else plain text."""
    data = out.get("data") if isinstance(out, dict) else None
    if not isinstance(data, dict):
        return ""
    for mime in ("image/png", "image/jpeg"):
        b64 = data.get(mime)
        if isinstance(b64, str) and b64:
            return f'<img src="data:{mime};base64,{b64}" style="max-width:100%">'
    for mime in ("image/svg+xml", "text/html"):
        markup = data.get(mime)
        if isinstance(markup, str) and markup:
            return markup
    text = data.get("text/plain")
    return f"<pre>{html.escape(text)}</pre>" if isinstance(text, str) and text else ""


def _render_outputs(outputs: list) -> str:
    """A list of mime bundles as one HTML fragment, each output in its own block."""
    blocks = [rendered for out in (outputs or []) if (rendered := _output_html(out))]
    return "".join(f'<div style="margin:6px 0">{b}</div>' for b in blocks)


def _has_view(out: dict) -> bool:
    data = out.get("data") if isinstance(out, dict) else None
    return isinstance(data, dict) and IX_VIEW_MIME in data


def _view_spec(outputs: list) -> dict | None:
    """The first structured-view spec (``{"renderer", "data"}``) carried by an
    output's ``IX_VIEW_MIME``, or None. The store JSON-encodes custom mimes, so
    the spec arrives as a string here."""
    for out in outputs or []:
        data = out.get("data") if isinstance(out, dict) else None
        if not isinstance(data, dict):
            continue
        spec = data.get(IX_VIEW_MIME)
        if isinstance(spec, str):
            try:
                spec = json.loads(spec)
            except json.JSONDecodeError:
                spec = None
        if isinstance(spec, dict) and isinstance(spec.get("renderer"), str) and "data" in spec:
            return spec
    return None


def _is_rich(out: dict) -> bool:
    """Whether an output deserves its own pane: a table/plot/HTML, not the plain
    text (and the internal ix mime bundles) the exec pane already shows."""
    data = out.get("data") if isinstance(out, dict) else None
    if not isinstance(data, dict):
        return False
    return any(
        mime != "text/plain" and not mime.startswith("application/x-ix") for mime in data
    )


def _panes(conn: sqlite3.Connection) -> list[dict]:
    """The MCP's current pane set, mapped from the store."""
    panes: list[dict] = []
    # A reserved pane carrying this session's identity. It rides under this
    # producer's scope like every other pane, so the dashboard reads its title to
    # label this session in its selector; it is not a run, so the feed excludes it
    # (the same treatment the namespace pane gets).
    sess = store.get_session(conn)
    if sess and (sess.get("name") or sess.get("client")):
        panes.append(
            data_pane(
                "__session__",
                sess.get("name") or "session",
                "session",
                {"name": sess.get("name") or "", "client": sess.get("client") or ""},
            )
        )
    # `recent` is newest-first; reverse so the feed (oldest-first) grows downward
    # like a log, matching how the board stamps and orders first appearances.
    for row in reversed(store.recent(conn, limit=_JOBS_LIMIT)):
        status = row.get("status") or "done"
        started = row.get("started_at")
        ended = row.get("ended_at")
        duration_ms = round((ended - started) * 1000) if ended and started else None
        # The run's intent (its human title). The kernel defaults a nameless job's
        # `name` to the run id, so treat name==id as "no intent" and pass None —
        # exec_pane then titles the run by its first source line, never a bare id.
        name = row.get("name")
        intent = name if name and name != row["id"] else None
        panes.append(
            exec_pane(
                row["id"],
                source=row.get("code") or "",
                running=status == "running",
                stdout=row.get("output") or "",
                stderr=row.get("error") or "",
                result=row.get("result") or "",
                ok=_ok(status),
                duration_ms=duration_ms,
                title=intent,
            )
        )
        # Rich outputs get their own pane beside the exec text. A structured
        # view spec (IX_VIEW_MIME) becomes a native `data` pane routed through
        # the frontend's renderer registry — but only when it is the run's SOLE
        # rich output: a run that also displayed a plot or table keeps the
        # sandboxed html pane so nothing beside the view is dropped (the view
        # bundle's text/html fallback renders it there).
        outputs = row.get("outputs") or []
        others_rich = any(_is_rich(out) for out in outputs if not _has_view(out))
        if not others_rich and (spec := _view_spec(outputs)) is not None:
            panes.append(
                data_pane(
                    f"{row['id']}/out",
                    intent or "output",
                    spec["renderer"],
                    spec["data"],
                    subtitle="output",
                )
            )
        elif any(_is_rich(out) for out in outputs) and (rendered := _render_outputs(outputs)):
            panes.append(
                html_pane(f"{row['id']}/out", intent or "output", rendered, subtitle="output")
            )
    # The agent's curated presentation cells (the highlight reel the old UI's
    # cells pane showed), each rendered as an html pane in position order.
    for cell in store.cells(conn):
        rendered = _render_outputs(cell.get("outputs") or [])
        if rendered:
            panes.append(
                html_pane(f"cell/{cell['id']}", cell.get("title") or "cell", rendered, subtitle="cell")
            )
    panes.extend(
        html_pane(
            f"resource/{res['id']}",
            res.get("title") or res["id"],
            res.get("html") or "",
            subtitle=res.get("kind") or "",
        )
        for res in store.live_resources(conn)
    )
    rows = store.latest_namespace(conn)
    if rows:
        panes.append(data_pane("namespace", "Namespace", "namespace", rows))
    return panes


async def run(store_path: str | Path, *, interval: float = _POLL_SECONDS) -> None:
    """Publish the store as panes until cancelled. Mirrors the store on every tick
    and pushes a new snapshot only when the rendered set changes."""
    producer = await PaneProducer().start()
    if producer is None:
        return
    conn = store.connect(store_path)
    last: str | None = None
    logged_error = False
    try:
        while True:
            try:
                panes = _panes(conn)
                # Cheap change check so an idle session never re-publishes.
                fingerprint = json.dumps(panes, separators=(",", ":"), sort_keys=True)
                if fingerprint != last:
                    await producer.publish(panes)
                    last = fingerprint
            except Exception as error:
                # A transient read error must not kill the bridge; try next tick.
                # Log the first one so a persistent failure (e.g. a schema
                # mismatch) is diagnosable instead of a silently empty dashboard.
                if not logged_error:
                    print(f"[ix-mcp] dashboard pane bridge error: {error!r}", flush=True)
                    logged_error = True
            await asyncio.sleep(interval)
    except asyncio.CancelledError:
        pass
    finally:
        await producer.stop()
        conn.close()
