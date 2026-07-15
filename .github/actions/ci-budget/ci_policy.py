from __future__ import annotations

import json
from dataclasses import dataclass
from math import ceil
from pathlib import Path
from collections.abc import Mapping
from datetime import datetime, timedelta

POLICY_PATH = Path(__file__).with_name("policy.json")


@dataclass(frozen=True)
class Policy:
    standard_seconds: int
    extended_validation_seconds: int
    extended_setup_allowance_seconds: int
    termination_grace_seconds: int

    @property
    def extended_worker_minutes(self) -> int:
        seconds = (
            self.extended_validation_seconds
            + self.extended_setup_allowance_seconds
            + self.termination_grace_seconds
        )
        return ceil(seconds / 60)


def positive_int(value: object, name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise RuntimeError(f"CI budget policy {name} must be a positive integer")
    return value


def load_policy(path: Path = POLICY_PATH) -> Policy:
    parsed = json.loads(path.read_text())
    if not isinstance(parsed, dict):
        raise RuntimeError("CI budget policy must be a JSON object")
    expected = {
        "standard_seconds",
        "extended_validation_seconds",
        "extended_setup_allowance_seconds",
        "termination_grace_seconds",
    }
    if set(parsed) != expected:
        raise RuntimeError(
            f"CI budget policy keys must be {sorted(expected)}, got {sorted(parsed)}"
        )
    return Policy(
        standard_seconds=positive_int(parsed["standard_seconds"], "standard_seconds"),
        extended_validation_seconds=positive_int(
            parsed["extended_validation_seconds"], "extended_validation_seconds"
        ),
        extended_setup_allowance_seconds=positive_int(
            parsed["extended_setup_allowance_seconds"],
            "extended_setup_allowance_seconds",
        ),
        termination_grace_seconds=positive_int(
            parsed["termination_grace_seconds"], "termination_grace_seconds"
        ),
    )


POLICY = load_policy()
STANDARD_BUDGET = timedelta(seconds=POLICY.standard_seconds)


def standard_minutes() -> int:
    return int(STANDARD_BUDGET.total_seconds() // 60)


def worker_timeout_minutes(*, big_change: bool) -> int:
    if big_change:
        return POLICY.extended_worker_minutes
    return standard_minutes()


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
