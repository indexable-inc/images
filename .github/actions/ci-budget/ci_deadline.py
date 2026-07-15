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
    GitHubClient,
    JsonObject,
    classify_workflow_attempt,
    load_canonical_globs,
    parse_bool,
    parse_globs,
    parse_positive_int,
)
from ci_policy import STANDARD_BUDGET, parse_timestamp


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
) -> None:
    attempt = client.workflow_attempt(run_id, run_attempt)
    started_at = parse_timestamp(attempt.get("run_started_at"), "run_started_at")
    deadline = started_at + budget
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
    if target_started_at < started_at:
        raise RuntimeError(
            f"required target {target_name!r} was reused from an earlier attempt; "
            f"target started at {target_started_at.isoformat()}, "
            f"attempt started at {started_at.isoformat()}"
        )
    completed_at = parse_timestamp(
        target.get("completed_at"), f"{target_name} completed_at"
    )
    if not big_change and completed_at > deadline:
        raise RuntimeError(
            f"required target {target_name!r} completed at "
            f"{completed_at.isoformat()}, after {deadline.isoformat()}"
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
    globs: Sequence[str],
    budget: timedelta,
    *,
    force_big_change: bool,
    merge_queue_branch: str,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    attempt = client.workflow_attempt(run_id, run_attempt)
    started_at = parse_timestamp(attempt.get("run_started_at"), "run_started_at")
    deadline = started_at + budget
    push_base_sha = None
    if not force_big_change and attempt.get("event") == "push":
        while push_base_sha is None:
            push_base_sha = client.ci_budget_context_base_sha(run_id, run_attempt)
            remaining = (deadline - now()).total_seconds()
            if push_base_sha is None and remaining <= 0:
                raise RuntimeError(
                    "source workflow did not publish its push base SHA before "
                    f"{deadline.isoformat()}"
                )
            if push_base_sha is None:
                sleep(min(2, remaining))
    classification = classify_workflow_attempt(
        client,
        attempt,
        globs,
        force_big_change=force_big_change,
        merge_queue_branch=merge_queue_branch,
        push_base_sha=push_base_sha,
    )
    if classification.big_change:
        print(json.dumps(classification.reason, separators=(",", ":")))
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
    client.cancel_workflow_run(run_id)
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
        globs = load_canonical_globs(Path(__file__).with_name("costly-paths"))
        globs.extend(
            parse_globs(
                os.environ["CI_BUDGET_EXTRA_COSTLY_PATHS"], "extra-costly-paths"
            )
        )
        cancel_at_deadline(
            client,
            run_id,
            run_attempt,
            globs,
            STANDARD_BUDGET,
            force_big_change=parse_bool(
                os.environ["CI_BUDGET_FORCE_BIG_CHANGE"], "force-big-change"
            ),
            merge_queue_branch=os.environ["CI_BUDGET_MERGE_QUEUE_BRANCH"],
        )
        return 0
    raise ValueError(f"unknown deadline mode {mode!r}")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-deadline::{error}", file=sys.stderr)
        sys.exit(1)
