#!/usr/bin/env python3
"""Verify required CI results and cancel ordinary runs at their deadline."""

from __future__ import annotations

import json
import os
import sys
import time
from collections.abc import Callable, Sequence
from datetime import UTC, datetime, timedelta
from pathlib import Path

from ci_budget import (
    BudgetSnapshot,
    GitHubClient,
    JsonObject,
    parse_bool,
    parse_positive_int,
)
from ci_policy import STANDARD_BUDGET, parse_timestamp
from workflow_cancellation import (
    Canceller,
    CancellationReason,
    CancellationReasonCode,
    CancellationSource,
    CancellationSourceKind,
    WorkflowCancellation,
    WorkflowCanceller,
    source_from_environment,
)


def target_job(jobs: Sequence[JsonObject], target_name: str) -> JsonObject:
    matches = [job for job in jobs if job.get("name") == target_name]
    if len(matches) != 1:
        raise RuntimeError(
            f"workflow attempt has {len(matches)} jobs named {target_name!r}; "
            "expected exactly one"
        )
    return matches[0]


def verify_required_gate(
    client: GitHubClient,
    run_id: int,
    run_attempt: int,
    target_name: str,
    budget: timedelta,
    *,
    big_change: bool,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
) -> None:
    attempt = client.workflow_attempt(run_id, run_attempt)
    created_at = parse_timestamp(attempt.get("created_at"), "created_at")
    attempt_started_at = parse_timestamp(
        attempt.get("run_started_at"), "run_started_at"
    )
    deadline = created_at + budget
    target = target_job(client.workflow_jobs(run_id, run_attempt), target_name)
    status = target.get("status")
    conclusion = target.get("conclusion")
    if status != "completed" or conclusion != "success":
        raise RuntimeError(
            f"required target {target_name!r} ended with "
            f"status={status!r}, conclusion={conclusion!r}"
        )
    target_started_at = parse_timestamp(
        target.get("started_at"), f"{target_name} started_at"
    )
    if target_started_at < attempt_started_at:
        raise RuntimeError(
            f"required target {target_name!r} was reused from an earlier attempt; "
            f"target started at {target_started_at.isoformat()}, "
            f"attempt started at {attempt_started_at.isoformat()}"
        )
    completed_at = parse_timestamp(
        target.get("completed_at"), f"{target_name} completed_at"
    )
    if not big_change and completed_at > deadline:
        raise RuntimeError(
            f"required target {target_name!r} completed at "
            f"{completed_at.isoformat()}, after {deadline.isoformat()}"
        )
    checked_at = now()
    if not big_change and checked_at > deadline:
        raise RuntimeError(
            f"required terminal gate ran at {checked_at.isoformat()}, "
            f"after {deadline.isoformat()}"
        )
    print(
        json.dumps(
            {
                "big_change": big_change,
                "completed_at": completed_at.isoformat(),
                "deadline": deadline.isoformat(),
                "target": target_name,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )


def cancel_at_deadline(
    client: GitHubClient,
    canceller: Canceller,
    repository: str,
    run_id: int,
    run_attempt: int,
    budget: timedelta,
    *,
    source: CancellationSource,
    force_big_change: bool,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    attempt = client.workflow_attempt(run_id, run_attempt)
    created_at = parse_timestamp(attempt.get("created_at"), "created_at")
    deadline = created_at + budget
    if force_big_change:
        return False
    status = attempt.get("status")
    if not isinstance(status, str):
        raise RuntimeError("GitHub API workflow attempt has no status")
    if status == "completed":
        print("workflow attempt completed before its budget controller started")
        return False

    snapshot = None
    while snapshot is None:
        snapshot = client.ci_budget_snapshot(run_id, run_attempt)
        remaining = (deadline - now()).total_seconds()
        if snapshot is None and remaining <= 0:
            refreshed = client.workflow_attempt(run_id, run_attempt)
            if refreshed.get("status") == "completed":
                print("workflow attempt completed before its budget snapshot")
                return False
            print(
                "::error title=CI total deadline exceeded::"
                "source workflow did not publish its budget snapshot before "
                f"{deadline.isoformat()}"
            )
            canceller.cancel(
                deadline_cancellation(
                    repository,
                    run_id,
                    run_attempt,
                    source,
                    deadline,
                )
            )
            return True
        if snapshot is None:
            sleep(min(2, remaining))
    if snapshot.big_change:
        print('{"big_change":true,"source":"attempt_snapshot"}')
        return False

    remaining = (deadline - now()).total_seconds()
    if remaining > 0:
        sleep(remaining)

    refreshed = client.workflow_attempt(run_id, run_attempt)
    status = refreshed.get("status")
    if not isinstance(status, str):
        raise RuntimeError("GitHub API workflow attempt has no status")
    if status == "completed":
        print(
            f"workflow attempt completed before cancellation at {deadline.isoformat()}"
        )
        return False

    print(
        f"::error title=CI total deadline exceeded::"
        f"cancelling run {run_id} attempt {run_attempt} at {deadline.isoformat()}"
    )
    canceller.cancel(
        deadline_cancellation(
            repository,
            run_id,
            run_attempt,
            source,
            deadline,
        )
    )
    return True


def deadline_cancellation(
    repository: str,
    run_id: int,
    run_attempt: int,
    source: CancellationSource,
    deadline: datetime,
) -> WorkflowCancellation:
    return WorkflowCancellation(
        repository=repository,
        run_id=run_id,
        run_attempt=run_attempt,
        reason=CancellationReason(
            code=CancellationReasonCode.CI_TOTAL_DEADLINE_EXCEEDED,
            detail=(
                f"ordinary CI exceeded its {int(STANDARD_BUDGET.total_seconds())} "
                f"second total budget at {deadline.isoformat()}"
            ),
        ),
        source=source,
    )


def main() -> int:
    repository = os.environ["CI_BUDGET_REPOSITORY"]
    client = GitHubClient(repository, os.environ["CI_BUDGET_TOKEN"])
    mode = os.environ["CI_BUDGET_DEADLINE_MODE"]
    run_id = parse_positive_int(os.environ["CI_BUDGET_RUN_ID"], "run-id")
    run_attempt = parse_positive_int(os.environ["CI_BUDGET_RUN_ATTEMPT"], "run-attempt")
    if mode == "gate":
        target_name = os.environ["CI_BUDGET_TARGET_JOB_NAME"]
        if not target_name:
            raise ValueError("target-job-name must not be empty")
        verify_required_gate(
            client,
            run_id,
            run_attempt,
            target_name,
            STANDARD_BUDGET,
            big_change=parse_bool(os.environ["CI_BUDGET_BIG_CHANGE"], "big-change"),
        )
        return 0
    if mode == "cancel":
        source = source_from_environment(
            CancellationSourceKind.CI_DEADLINE_CONTROLLER,
            os.environ,
        )
        canceller = WorkflowCanceller(
            client.request,
            repository,
            Path(os.environ["WORKFLOW_CANCELLATION_RECORD_DIRECTORY"]),
            Path(os.environ["GITHUB_STEP_SUMMARY"]),
        )
        cancel_at_deadline(
            client,
            canceller,
            repository,
            run_id,
            run_attempt,
            STANDARD_BUDGET,
            source=source,
            force_big_change=parse_bool(
                os.environ["CI_BUDGET_FORCE_BIG_CHANGE"], "force-big-change"
            ),
        )
        return 0
    raise ValueError(f"unknown deadline mode {mode!r}")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-deadline::{error}", file=sys.stderr)
        sys.exit(1)
