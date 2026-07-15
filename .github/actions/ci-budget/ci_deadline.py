#!/usr/bin/env python3
"""Verify required CI results and cancel ordinary runs at their deadline."""

from __future__ import annotations

import json
import os
import sys
import time
from collections.abc import Callable, Sequence
from datetime import UTC, datetime, timedelta

from ci_budget import (
    BudgetSnapshot,
    GitHubClient,
    GitHubTransportError,
    JsonObject,
    RequestWindowExpired,
    parse_bool,
    parse_positive_int,
)
from ci_policy import STANDARD_BUDGET, TERMINATION_GRACE, parse_timestamp

CANCELLATION_REQUEST_TIMEOUT_SECONDS = 10.0
TERMINAL_POLL_SECONDS = 1.0


def request_timeout_before(
    cutoff: datetime,
    now: Callable[[], datetime],
) -> float:
    remaining = (cutoff - now()).total_seconds()
    if remaining <= 0:
        raise RequestWindowExpired(
            f"GitHub API request window ended at {cutoff.isoformat()}"
        )
    return min(CANCELLATION_REQUEST_TIMEOUT_SECONDS, remaining)


def cancel_and_wait_for_terminal(
    client: GitHubClient,
    run_id: int,
    deadline: datetime,
    *,
    now: Callable[[], datetime],
    sleep: Callable[[float], None],
) -> None:
    missed_deadline = False
    cancellation_accepted = False
    while True:
        if now() >= deadline and not missed_deadline:
            missed_deadline = True
            cancellation_accepted = False
            print(
                "::error title=CI terminal deadline missed::"
                f"workflow run {run_id} was not confirmed terminal by "
                f"{deadline.isoformat()}; continuing force cancellation"
            )
        try:
            timeout_seconds = (
                CANCELLATION_REQUEST_TIMEOUT_SECONDS
                if missed_deadline
                else request_timeout_before(deadline, now)
            )
        except RequestWindowExpired:
            continue
        if not cancellation_accepted:
            try:
                client.force_cancel_workflow_run(
                    run_id,
                    timeout_seconds=timeout_seconds,
                )
                cancellation_accepted = True
            except GitHubTransportError as error:
                print(
                    f"::warning title=CI force cancellation transport failed::{error}"
                )
        if now() >= deadline and not missed_deadline:
            continue
        try:
            timeout_seconds = (
                CANCELLATION_REQUEST_TIMEOUT_SECONDS
                if missed_deadline
                else request_timeout_before(deadline, now)
            )
        except RequestWindowExpired:
            continue
        try:
            run = client.workflow_run(run_id, timeout_seconds=timeout_seconds)
        except GitHubTransportError as error:
            print(f"::warning title=CI terminal check transport failed::{error}")
        else:
            observed_at = now()
            status = run.get("status")
            if not isinstance(status, str):
                raise RuntimeError("GitHub API workflow run has no status")
            if status == "completed":
                if missed_deadline or observed_at > deadline:
                    raise RuntimeError(
                        f"workflow run {run_id} was first confirmed terminal at "
                        f"{observed_at.isoformat()}, after {deadline.isoformat()}"
                    )
                return
        if missed_deadline:
            cancellation_accepted = False
            sleep(TERMINAL_POLL_SECONDS)
            continue
        remaining = (deadline - now()).total_seconds()
        if remaining > 0:
            sleep(min(TERMINAL_POLL_SECONDS, remaining))


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
    run = client.workflow_run(run_id)
    attempt = client.workflow_attempt(run_id, run_attempt)
    created_at = parse_timestamp(run.get("created_at"), "created_at")
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
    run_id: int,
    run_attempt: int,
    budget: timedelta,
    *,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    run = client.workflow_run(
        run_id,
        timeout_seconds=CANCELLATION_REQUEST_TIMEOUT_SECONDS,
    )
    created_at = parse_timestamp(run.get("created_at"), "created_at")
    deadline = created_at + budget
    cancel_at = deadline - TERMINATION_GRACE
    status = run.get("status")
    if not isinstance(status, str):
        raise RuntimeError("GitHub API workflow attempt has no status")
    if status == "completed":
        print("workflow attempt completed before its budget controller started")
        return False

    snapshot = None
    while snapshot is None:
        try:
            snapshot = client.ci_budget_snapshot(
                run_id,
                run_attempt,
                request_timeout=lambda: request_timeout_before(cancel_at, now),
            )
        except RequestWindowExpired:
            snapshot = None
        except GitHubTransportError as error:
            print(f"::warning title=CI budget snapshot transport failed::{error}")
            snapshot = None
        remaining = (cancel_at - now()).total_seconds()
        if remaining <= 0:
            print(
                "::error title=CI total deadline exceeded::"
                "source workflow budget was not confirmed before "
                f"cancellation started at {cancel_at.isoformat()}"
            )
            cancel_and_wait_for_terminal(client, run_id, deadline, now=now, sleep=sleep)
            return True
        if snapshot is None:
            sleep(min(2, remaining))
    if snapshot.big_change:
        print('{"big_change":true,"source":"attempt_snapshot"}')
        return False

    remaining = (cancel_at - now()).total_seconds()
    if remaining > 0:
        sleep(remaining)

    print(
        f"::error title=CI total deadline exceeded::"
        f"cancelling run {run_id} attempt {run_attempt} at {cancel_at.isoformat()} "
        f"to reserve termination through {deadline.isoformat()}"
    )
    cancel_and_wait_for_terminal(client, run_id, deadline, now=now, sleep=sleep)
    return True


def main() -> int:
    client = GitHubClient(
        os.environ["CI_BUDGET_REPOSITORY"], os.environ["CI_BUDGET_TOKEN"]
    )
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
        cancel_at_deadline(
            client,
            run_id,
            run_attempt,
            STANDARD_BUDGET,
        )
        return 0
    raise ValueError(f"unknown deadline mode {mode!r}")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-deadline::{error}", file=sys.stderr)
        sys.exit(1)
