"""Export this store's tool usage for the ix desktop-space graph vault.

The vault (``ix-desktop-space --tools <file>``) renders each capability an
agent has used as a small crystal node linked to that agent, and wants one
JSON document in its ``ToolUsage`` shape::

    {"tools": [{"id", "label", "kind", "description"}],
     "links": [{"agent", "tool", "calls", "last_at"}]}

Everything needed already exists here: :mod:`.registry` is the catalog of
capability modules (name + tagline), and the ``executions`` table records
every ``python_exec`` run with the names its cell touched (``bindings``,
introspected at finish; the cell source is the fallback for runs that never
finished). This module only joins the two — one run that touched a module
counts as one call to that capability — so the registry and the execution
log stay the single sources of truth.

Usage counts are per store file: one store == one MCP session == one agent.
The exporter stamps that agent id on every link; the vault drops links whose
agent is not in its federated graph, so an id must match the graph's
``ix://<host>/<name>`` uri for the tools to appear.
"""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from . import introspect, registry, store


def export(conn: sqlite3.Connection, *, agent: str) -> dict:
    """Fold this store's executions into the vault ``ToolUsage`` document.

    A run "used" a capability module when the module's name appears among the
    names its cell bound or referenced: the stored ``bindings`` when the run
    finished, else a fresh parse of its source (:func:`introspect.binding_names`
    returns empty sets for code that does not parse, so a broken cell simply
    counts nothing). ``last_at`` is the run's end (or start) in unix millis,
    matching the vault's timestamp unit.
    """
    modules = {m.name: m for m in registry.MODULES}
    calls: dict[str, int] = {}
    last: dict[str, int] = {}
    for run in store.recent(conn, limit=100_000):
        used = set((run.get("bindings") or {}).keys())
        if not used:
            assigned, loads = introspect.binding_names(run.get("code") or "")
            used = assigned | loads
        at = run.get("ended_at") or run.get("started_at")
        for name in used & modules.keys():
            calls[name] = calls.get(name, 0) + 1
            if at:
                last[name] = max(last.get(name, 0), int(at * 1000))
    return {
        "tools": [
            {
                "id": f"tool/mcp/{name}",
                "label": name,
                "kind": "mcp",
                "description": modules[name].tagline,
            }
            for name in sorted(calls)
        ],
        "links": [
            {
                "agent": agent,
                "tool": f"tool/mcp/{name}",
                "calls": count,
                "last_at": last.get(name),
            }
            for name, count in sorted(calls.items())
        ],
    }


def export_stores(paths: list[tuple[str, Path]]) -> dict:
    """Merge several ``(agent, store file)`` pairs into one document.

    Tools dedupe on id (they all come from the same registry); links stay
    per-agent so the vault can draw one spoke per calling agent.
    """
    tools: dict[str, dict] = {}
    links: list[dict] = []
    for agent, path in paths:
        conn = sqlite3.connect(path)
        try:
            doc = export(conn, agent=agent)
        finally:
            conn.close()
        for tool in doc["tools"]:
            tools[tool["id"]] = tool
        links.extend(doc["links"])
    return {"tools": sorted(tools.values(), key=lambda t: t["id"]), "links": links}


def main_export(specs: list[str], out: Path | None) -> int:
    """CLI body for ``ix-mcp toolusage AGENT=STORE [...]``: write (or print)
    the merged document. A malformed spec is a usage error, not a crash."""
    pairs: list[tuple[str, Path]] = []
    for spec in specs:
        agent, sep, path = spec.partition("=")
        if not sep or not agent or not path:
            print(f"toolusage: bad spec {spec!r} (want AGENT=STORE.sqlite)")
            return 2
        pairs.append((agent, Path(path)))
    doc = export_stores(pairs)
    text = json.dumps(doc, indent=1)
    if out is None:
        print(text)
    else:
        out.write_text(text)
    return 0
