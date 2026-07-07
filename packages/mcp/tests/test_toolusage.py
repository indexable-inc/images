"""The vault export: executions × registry → ToolUsage JSON.

Defends the join contract the ix desktop-space graph vault consumes: one run
that touched a capability module counts as one call, unfinished runs fall
back to parsing their source, non-module names never leak into the catalog,
and timestamps come out in unix millis.
"""

import json
import sqlite3
from pathlib import Path

from ix_notebook_mcp import store, toolusage


def _mkstore(path: Path) -> sqlite3.Connection:
    return store.connect(path)


def _run(
    conn: sqlite3.Connection,
    id: str,
    code: str,
    *,
    bindings: dict | None = None,
    started_at: float = 100.0,
    finish: bool = True,
) -> None:
    store.start(conn, id=id, name="run", code=code, started_at=started_at)
    if finish:
        store.finish(
            conn,
            id=id,
            status="done",
            ended_at=started_at + 1,
            output="",
            result=None,
            error=None,
            bindings=bindings,
        )


def test_export_counts_module_usage(tmp_path: Path) -> None:
    conn = _mkstore(tmp_path / "s.sqlite")
    # two finished runs used `fleet` (via recorded bindings), one used `search`.
    _run(conn, "a", "fleet.deploy()", bindings={"fleet": {}}, started_at=100)
    _run(conn, "b", "fleet.status()", bindings={"fleet": {}}, started_at=200)
    # `df` is a plain variable, not a registry module (unlike `x`, which IS one).
    _run(conn, "c", "df = search.web('q')", bindings={"search": {}, "df": {}}, started_at=300)
    doc = toolusage.export(conn, agent="ix://h/ada")

    by_tool = {l["tool"]: l for l in doc["links"]}
    assert by_tool["tool/mcp/fleet"]["calls"] == 2
    assert by_tool["tool/mcp/search"]["calls"] == 1
    assert by_tool["tool/mcp/fleet"]["last_at"] == 201_000, "unix millis, latest run"
    assert all(l["agent"] == "ix://h/ada" for l in doc["links"])
    # catalog carries the registry tagline, and only for used modules.
    tools = {t["id"]: t for t in doc["tools"]}
    assert set(tools) == {"tool/mcp/fleet", "tool/mcp/search"}
    assert tools["tool/mcp/fleet"]["description"], "tagline from the registry"
    # `df` is not a registry module and must not leak into the catalog.
    assert "tool/mcp/df" not in tools


def test_unfinished_run_falls_back_to_source_parse(tmp_path: Path) -> None:
    conn = _mkstore(tmp_path / "s.sqlite")
    # still running: no bindings recorded yet, so the source parse attributes it.
    _run(conn, "a", "import mesh\nmesh.peers()", started_at=50, finish=False)
    doc = toolusage.export(conn, agent="ix://h/ada")
    assert [l["tool"] for l in doc["links"]] == ["tool/mcp/mesh"]
    assert doc["links"][0]["calls"] == 1


def test_export_stores_merges_agents(tmp_path: Path) -> None:
    a = tmp_path / "a.sqlite"
    b = tmp_path / "b.sqlite"
    _run(_mkstore(a), "r1", "fleet.x", bindings={"fleet": {}})
    _run(_mkstore(b), "r2", "fleet.y", bindings={"fleet": {}})
    doc = toolusage.export_stores([("ix://h/ada", a), ("ix://h/grace", b)])
    assert len(doc["tools"]) == 1, "tools dedupe on id"
    assert {(l["agent"], l["calls"]) for l in doc["links"]} == {
        ("ix://h/ada", 1),
        ("ix://h/grace", 1),
    }


def test_cli_writes_file(tmp_path: Path) -> None:
    s = tmp_path / "s.sqlite"
    _run(_mkstore(s), "r", "view.ls('.')", bindings={"view": {}})
    out = tmp_path / "tools.json"
    rc = toolusage.main_export([f"ix://h/ada={s}"], out)
    assert rc == 0
    doc = json.loads(out.read_text())
    assert doc["links"][0]["tool"] == "tool/mcp/view"
    # a malformed spec is a usage error, not a crash.
    assert toolusage.main_export(["nonsense"], None) == 2
