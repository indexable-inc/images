"""Load the versioned eval sets from JSONL.

The datasets are data, not code: one JSON object per line so a diff shows
exactly which query or task changed. See ``datasets/retrieval.jsonl`` and
``datasets/tasks.jsonl``.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .model import RetrievalCase, TaskCase
from .paths import dataset_path


def _read_lines(name: str, override: Path | None) -> list[dict[str, Any]]:
    """Read a JSONL dataset, from an explicit path or the packaged default."""
    path = override if override is not None else dataset_path(name)
    rows: list[dict[str, Any]] = []
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError as exc:
            raise ValueError(f"{name}:{lineno}: invalid JSON: {exc}") from exc
    return rows


def load_retrieval(override: Path | None = None) -> list[RetrievalCase]:
    """Load the Tier A retrieval cases."""
    cases: list[RetrievalCase] = []
    for row in _read_lines("retrieval.jsonl", override):
        relevant = {str(k): float(v) for k, v in dict(row["relevant"]).items()}
        cases.append(
            RetrievalCase(id=str(row["id"]), query=str(row["query"]), relevant=relevant)
        )
    return cases


def load_tasks(override: Path | None = None) -> list[TaskCase]:
    """Load the Tier B agentic task cases."""
    return [
        TaskCase(id=str(row["id"]), task=str(row["task"]), answer=str(row["answer"]))
        for row in _read_lines("tasks.jsonl", override)
    ]
