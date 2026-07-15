from __future__ import annotations

from collections.abc import Mapping
from datetime import datetime, timedelta

STANDARD_BUDGET = timedelta(minutes=5)


def standard_minutes() -> int:
    return int(STANDARD_BUDGET.total_seconds() // 60)


def parse_timestamp(value: object, name: str) -> datetime:
    if not isinstance(value, str):
        raise RuntimeError(f"GitHub API result has no {name}")
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise RuntimeError(f"GitHub API result {name} has no timezone")
    return parsed


def standard_deadline(attempt: Mapping[str, object]) -> datetime:
    return (
        parse_timestamp(attempt.get("run_started_at"), "run_started_at")
        + STANDARD_BUDGET
    )
