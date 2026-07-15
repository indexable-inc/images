#!/usr/bin/env python3
"""Cancel ordinary CI that exceeds its total workflow-start budget."""

from __future__ import annotations

import json
import os
import sys
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta

from ci_budget import GitHubClient, JsonObject
from ci_policy import STANDARD_BUDGET

POLL_SECONDS = 10


@dataclass(frozen=True)
class TargetState:
    complete: bool
    late: tuple[str, ...]
    missing: tuple[str, ...]
    pending: tuple[str, ...]


def parse_positive_int(value: str, name: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise ValueError(f"{name} must be positive")
    return parsed


def parse_job_names(value: str) -> list[str]:
    decoded = json.loads(value)
    if (
        not isinstance(decoded, list)
        or not decoded
        or not all(isinstance(item, str) and item for item in decoded)
    ):
        raise ValueError("target-job-names must be a non-empty JSON array of strings")
    if len(set(decoded)) != len(decoded):
        raise ValueError("target-job-names must not contain duplicates")
    return decoded


def parse_timestamp(value: object, name: str) -> datetime:
    if not isinstance(value, str):
        raise RuntimeError(f"GitHub API workflow attempt has no {name}")
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise RuntimeError(f"GitHub API workflow attempt {name} has no timezone")
    return parsed


def target_state(
    jobs: Sequence[JsonObject], target_names: Sequence[str], deadline: datetime
) -> TargetState:
    statuses: dict[str, str] = {}
    late: list[str] = []
    for job in jobs:
        name = job.get("name")
        status = job.get("status")
        if name in target_names:
            if name in statuses:
                raise RuntimeError(
                    f"workflow attempt has duplicate target job {name!r}"
                )
            if not isinstance(status, str):
                raise RuntimeError(f"target job {name!r} has no status")
            statuses[name] = status
            if status == "completed":
                completed_at = parse_timestamp(
                    job.get("completed_at"), f"{name} completed_at"
                )
                if completed_at > deadline:
                    late.append(name)
    missing = tuple(name for name in target_names if name not in statuses)
    pending = tuple(name for name, status in statuses.items() if status != "completed")
    return TargetState(
        complete=not late and not missing and not pending,
        late=tuple(late),
        missing=missing,
        pending=pending,
    )


def enforce(
    client: GitHubClient,
    run_id: int,
    run_attempt: int,
    target_names: Sequence[str],
    budget: timedelta,
    *,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    attempt = client.workflow_attempt(run_id, run_attempt)
    started_at = parse_timestamp(attempt.get("run_started_at"), "run_started_at")
    deadline = started_at + budget
    while True:
        state = target_state(
            client.workflow_jobs(run_id, run_attempt), target_names, deadline
        )
        if state.complete:
            print(f"CI targets completed before {deadline.isoformat()}")
            return False
        remaining = (deadline - now()).total_seconds()
        if remaining <= 0:
            detail = {
                "late": state.late,
                "missing": state.missing,
                "pending": state.pending,
            }
            print(f"::error title=CI total deadline exceeded::{json.dumps(detail)}")
            client.cancel_workflow_run(run_id)
            return True
        sleep(min(POLL_SECONDS, remaining))


def main() -> int:
    client = GitHubClient(
        os.environ["CI_BUDGET_REPOSITORY"], os.environ["CI_BUDGET_TOKEN"]
    )
    cancelled = enforce(
        client,
        parse_positive_int(os.environ["CI_BUDGET_RUN_ID"], "run-id"),
        parse_positive_int(os.environ["CI_BUDGET_RUN_ATTEMPT"], "run-attempt"),
        parse_job_names(os.environ["CI_BUDGET_TARGET_JOB_NAMES"]),
        STANDARD_BUDGET,
    )
    return int(cancelled)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-deadline::{error}", file=sys.stderr)
        sys.exit(1)
