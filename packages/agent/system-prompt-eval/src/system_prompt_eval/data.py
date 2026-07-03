"""Load the versioned behavior catalog and task set from JSONL.

The datasets are data, not code: one JSON object per line so a diff shows exactly
which behavior rubric or task changed. See ``datasets/behaviors.jsonl`` and
``datasets/tasks.jsonl``.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .model import Behavior, TaskCase
from .paths import dataset_path


def _read_lines(name: str, override: Path | None) -> list[dict[str, Any]]:
    """Read a JSONL dataset, from an explicit path or the packaged default."""
    path = override if override is not None else dataset_path(name)
    rows: list[dict[str, Any]] = []
    for lineno, raw_line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        line = raw_line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError as exc:
            raise ValueError(f"{name}:{lineno}: invalid JSON: {exc}") from exc
    return rows


def load_behaviors(override: Path | None = None) -> list[Behavior]:
    """Load the behavior catalog (id, name, scoring rubric)."""
    return [
        Behavior(id=str(row["id"]), name=str(row["name"]), rubric=str(row["rubric"]))
        for row in _read_lines("behaviors.jsonl", override)
    ]


def load_tasks(override: Path | None = None) -> list[TaskCase]:
    """Load the neutral task set, each tagged with the behaviors it should surface."""
    return [
        TaskCase(
            id=str(row["id"]),
            task=str(row["task"]),
            expects=tuple(str(b) for b in row["expects"]),
        )
        for row in _read_lines("tasks.jsonl", override)
    ]


def validate_expects(tasks: list[TaskCase], behaviors: list[Behavior]) -> None:
    """Fail loudly if a task's ``expects`` names a behavior id absent from the catalog.

    A typo'd or stale id would otherwise be silently dropped where ``expects`` is
    consumed (the runner scores only ids present in the catalog), masking dataset
    drift as a quietly lower score instead of a load-time error.
    """
    known = {b.id for b in behaviors}
    unknown = sorted(
        {bid for task in tasks for bid in task.expects if bid not in known}
    )
    if unknown:
        raise ValueError(
            f"tasks.jsonl expects unknown behavior id(s) not in behaviors.jsonl: "
            f"{', '.join(unknown)}"
        )
