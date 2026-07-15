from __future__ import annotations

from datetime import timedelta

STANDARD_BUDGET = timedelta(minutes=5)


def standard_minutes() -> int:
    return int(STANDARD_BUDGET.total_seconds() // 60)
