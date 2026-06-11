"""Example task-dependency graphs, generated in Python and stored in SQLite.

Bundled like ``view``/``sh``/``fff`` so every session can ``import tasks`` with no
setup. It is the single source of truth for the task-graph demo site
(``packages/mcp/task-graph``): Python generates a ~100-node dependency DAG and
writes it to a SQLite file, and the website reads that very file (via sql.js) and
draws it. One schema, one generator, two consumers.

    import tasks

    tasks.seed("tasks.sqlite")          # generate a 100-task DAG -> SQLite file
    rows = tasks.load("tasks.sqlite")   # read it back as a list[Task]
    tasks.frame("tasks.sqlite")         # a polars DataFrame (styled table in the dashboard)

The data model is deliberately tiny and acyclic: every task only ever depends on
tasks defined before it, so the dependency edges form a DAG. ``status_of`` derives
a task's state (done / in-progress / ready / blocked) from whether its
dependencies are complete, the same way the website colors its nodes.

Pure standard library at the core (``sqlite3``, ``dataclasses``); ``polars`` is
imported lazily only for :func:`frame`, and the kernel's ``Result`` is optional,
so this module also works as a plain ``import tasks`` outside the MCP kernel.
"""

from __future__ import annotations

import sqlite3
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Literal

__all__ = [
    "Task",
    "Status",
    "Category",
    "CATEGORIES",
    "STATUS_META",
    "CATEGORY_COLORS",
    "generate",
    "status_of",
    "write",
    "read",
    "load",
    "seed",
    "frame",
    "SCHEMA",
]

Category = Literal[
    "Design", "Backend", "Frontend", "Infra", "Data", "QA", "Docs"
]
Status = Literal["done", "in-progress", "ready", "blocked"]

CATEGORIES: tuple[Category, ...] = (
    "Design", "Backend", "Frontend", "Infra", "Data", "QA", "Docs",
)

# Status label + color, mirrored by the website's legend.
STATUS_META: dict[Status, dict[str, str]] = {
    "done": {"label": "Done", "color": "#3fb950"},
    "in-progress": {"label": "In progress", "color": "#58a6ff"},
    "ready": {"label": "Ready", "color": "#d29922"},
    "blocked": {"label": "Blocked", "color": "#f85149"},
}

CATEGORY_COLORS: dict[Category, str] = {
    "Design": "#bc8cff",
    "Backend": "#58a6ff",
    "Frontend": "#3fb950",
    "Infra": "#f0883e",
    "Data": "#39c5cf",
    "QA": "#f85149",
    "Docs": "#d2a8ff",
}

# Verb/noun pools per category so generated titles read like real work.
_TITLES: dict[Category, tuple[tuple[str, ...], tuple[str, ...]]] = {
    "Design": (
        ("Sketch", "Wireframe", "Prototype", "Audit", "Refine", "Spec"),
        ("onboarding flow", "dashboard layout", "icon set", "color system",
         "empty states", "settings panel"),
    ),
    "Backend": (
        ("Implement", "Refactor", "Optimize", "Expose", "Cache", "Validate"),
        ("auth service", "billing API", "user model", "search endpoint",
         "webhook queue", "rate limiter"),
    ),
    "Frontend": (
        ("Build", "Wire up", "Style", "Animate", "Componentize", "Localize"),
        ("login screen", "graph view", "task list", "command palette",
         "sidebar", "toast system"),
    ),
    "Infra": (
        ("Provision", "Harden", "Automate", "Monitor", "Scale", "Migrate"),
        ("CI pipeline", "k8s cluster", "secrets vault", "CDN edge",
         "backup job", "staging env"),
    ),
    "Data": (
        ("Model", "Ingest", "Clean", "Aggregate", "Index", "Backfill"),
        ("events table", "metrics rollup", "user graph", "audit log",
         "feature store", "embeddings"),
    ),
    "QA": (
        ("Write", "Automate", "Fuzz", "Load-test", "Triage", "Regress"),
        ("unit suite", "e2e flows", "API contract", "perf budget",
         "flaky tests", "smoke checks"),
    ),
    "Docs": (
        ("Document", "Diagram", "Record", "Publish", "Review", "Update"),
        ("API reference", "runbook", "architecture", "changelog",
         "tutorial", "onboarding guide"),
    ),
}


@dataclass(frozen=True, slots=True)
class Task:
    """One node in the dependency graph."""

    id: str
    title: str
    category: Category
    estimate: int  # days
    complete: bool
    active: bool
    deps: tuple[str, ...] = field(default_factory=tuple)


class _Rand:
    """mulberry32: a tiny, fast, seedable PRNG so a given seed always yields the
    same graph (matches the JS generator's algorithm)."""

    def __init__(self, seed: int) -> None:
        self._a = seed & 0xFFFFFFFF

    def next(self) -> float:
        self._a = (self._a + 0x6D2B79F5) & 0xFFFFFFFF
        t = self._a
        t = (t ^ (t >> 15)) * (t | 1) & 0xFFFFFFFF
        t ^= (t + ((t ^ (t >> 7)) * (t | 61) & 0xFFFFFFFF)) & 0xFFFFFFFF
        t &= 0xFFFFFFFF
        return ((t ^ (t >> 14)) & 0xFFFFFFFF) / 4294967296.0

    def below(self, n: int) -> int:
        return int(self.next() * n)

    def pick(self, seq: tuple[str, ...]) -> str:
        return seq[self.below(len(seq))]


def generate(count: int = 100, seed: int = 42) -> list[Task]:
    """Build ``count`` example tasks wired into a dependency DAG.

    Tasks only ever depend on earlier tasks, so the graph is acyclic. Completion
    is assigned consistently: a task can only be ``complete`` if all of its
    dependencies are complete, and earlier tasks are likelier to be done.
    """
    rand = _Rand(seed)
    used: set[str] = set()
    out: list[Task] = []

    for i in range(count):
        category = CATEGORIES[i % len(CATEGORIES)]
        verbs, nouns = _TITLES[category]

        title = ""
        for _ in range(12):
            title = f"{rand.pick(verbs)} {rand.pick(nouns)}"
            if title not in used:
                break
        if title in used:
            title = f"{title} #{i}"
        used.add(title)

        deps: list[str] = []
        # 0-2 dependencies, biased toward the most recent few tasks so the DAG
        # forms readable layers. Allowing 0 keeps a healthy supply of root tasks
        # (otherwise the whole graph blocks on a few unfinished ancestors).
        max_deps = 0 if i == 0 else rand.below(3)
        seen: set[int] = set()
        for _ in range(max_deps):
            window = min(i, 8)
            idx = i - 1 - rand.below(window)
            if idx >= 0 and idx not in seen:
                seen.add(idx)
                deps.append(out[idx].id)

        out.append(
            Task(
                id=f"T{i + 1:03d}",
                title=title,
                category=category,
                estimate=1 + rand.below(8),
                complete=False,
                active=False,
                deps=tuple(deps),
            )
        )

    # Assign completion in definition order (which is topological for a DAG whose
    # edges point backward), so done/blocked states stay consistent.
    by_id = {t.id: t for t in out}
    for i, task in enumerate(out):
        deps_done = all(by_id[d].complete for d in task.deps)
        if not deps_done:
            continue
        p = 0.9 * (1 - 0.2 * i / count)
        if rand.next() < p:
            by_id[task.id] = replace(task, complete=True)
        elif rand.next() < 0.5:
            by_id[task.id] = replace(task, active=True)

    return [by_id[t.id] for t in out]


def status_of(task: Task, by_id: dict[str, Task]) -> Status:
    """Derive a task's status from its dependencies' completion."""
    if task.complete:
        return "done"
    if not all(by_id[d].complete for d in task.deps):
        return "blocked"
    return "in-progress" if task.active else "ready"


# --- SQLite -----------------------------------------------------------------

SCHEMA = """
CREATE TABLE tasks (
  id        TEXT PRIMARY KEY,
  title     TEXT NOT NULL,
  category  TEXT NOT NULL,
  estimate  INTEGER NOT NULL,
  complete  INTEGER NOT NULL,
  active    INTEGER NOT NULL
);
CREATE TABLE deps (
  task_id    TEXT NOT NULL REFERENCES tasks(id),
  depends_on TEXT NOT NULL REFERENCES tasks(id),
  PRIMARY KEY (task_id, depends_on)
);
"""


def write(path: str | Path, rows: list[Task]) -> Path:
    """Write ``rows`` to a fresh SQLite database at ``path`` and return the path."""
    path = Path(path)
    if path.exists():
        path.unlink()
    con = sqlite3.connect(path)
    try:
        con.executescript(SCHEMA)
        con.executemany(
            "INSERT INTO tasks (id, title, category, estimate, complete, active)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            [
                (t.id, t.title, t.category, t.estimate, int(t.complete), int(t.active))
                for t in rows
            ],
        )
        con.executemany(
            "INSERT INTO deps (task_id, depends_on) VALUES (?, ?)",
            [(t.id, d) for t in rows for d in t.deps],
        )
        con.commit()
    finally:
        con.close()
    return path


def read(path: str | Path) -> list[Task]:
    """Read tasks (and their dependencies) back from a SQLite database."""
    con = sqlite3.connect(Path(path))
    try:
        con.row_factory = sqlite3.Row
        deps: dict[str, list[str]] = {}
        for r in con.execute("SELECT task_id, depends_on FROM deps"):
            deps.setdefault(r["task_id"], []).append(r["depends_on"])
        rows: list[Task] = []
        for r in con.execute(
            "SELECT id, title, category, estimate, complete, active FROM tasks"
            " ORDER BY id"
        ):
            rows.append(
                Task(
                    id=r["id"],
                    title=r["title"],
                    category=r["category"],
                    estimate=r["estimate"],
                    complete=bool(r["complete"]),
                    active=bool(r["active"]),
                    deps=tuple(deps.get(r["id"], ())),
                )
            )
        return rows
    finally:
        con.close()


# `load` is the read-oriented alias that reads naturally at a call site.
load = read


def seed(path: str | Path = "tasks.sqlite", count: int = 100, seed: int = 42) -> Path:
    """Generate a task DAG and write it to ``path``. Returns the SQLite path."""
    return write(path, generate(count, seed))


def frame(source: str | Path | list[Task]):
    """A ``polars.DataFrame`` of tasks with derived status, for the dashboard.

    ``source`` is a SQLite path or an already-loaded ``list[Task]``. Rendered as a
    styled HTML table in the MCP dashboard; a plain DataFrame elsewhere.
    """
    import polars as pl

    rows = source if isinstance(source, list) else read(source)
    by_id = {t.id: t for t in rows}
    return pl.DataFrame(
        {
            "id": [t.id for t in rows],
            "title": [t.title for t in rows],
            "category": [t.category for t in rows],
            "status": [status_of(t, by_id) for t in rows],
            "estimate": [t.estimate for t in rows],
            "deps": [len(t.deps) for t in rows],
        }
    )
