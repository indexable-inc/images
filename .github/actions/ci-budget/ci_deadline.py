#!/usr/bin/env python3
"""Verify required CI results and release attempts whose workers stayed queued."""

from __future__ import annotations

import json
import os
import sys
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta

from ci_budget import GitHubClient, JsonObject, parse_positive_int
from ci_policy import POLICY, parse_timestamp


@dataclass(frozen=True)
class WorkerTiming:
    name: str
    status: str
    created_at: datetime
    started_at: datetime | None
    queue_deadline: datetime

    @property
    def started_on_time(self) -> bool:
        return self.started_at is not None and self.started_at <= self.queue_deadline

    @property
    def outstanding(self) -> bool:
        return self.status != "completed"


def workflow_attempt(
    client: GitHubClient, run_id: int, run_attempt: int
) -> JsonObject:
    attempt = client.workflow_attempt(run_id, run_attempt)
    actual_attempt = attempt.get("run_attempt")
    if actual_attempt != run_attempt:
        raise RuntimeError(
            f"GitHub returned workflow attempt {actual_attempt!r}; "
            f"expected {run_attempt}"
        )
    return attempt


def target_job(jobs: Sequence[JsonObject], target_name: str) -> JsonObject:
    matches = [job for job in jobs if job.get("name") == target_name]
    if len(matches) != 1:
        raise RuntimeError(
            f"workflow attempt has {len(matches)} jobs named {target_name!r}; "
            "expected exactly one"
        )
    return matches[0]


def job_labels(job: JsonObject, name: str) -> tuple[str, ...]:
    labels = job.get("labels")
    if not isinstance(labels, list) or not all(
        isinstance(label, str) and label for label in labels
    ):
        raise RuntimeError(f"GitHub job {name!r} has malformed labels")
    return tuple(labels)


def worker_label_prefix(run_id: int, run_attempt: int) -> str:
    # indexable-inc/ix#7396 defines this as the dispatcher label contract.
    return f"ix-ci-run-{run_id}-{run_attempt}-"


def is_current_worker(job: JsonObject, run_id: int, run_attempt: int) -> bool:
    name = job.get("name")
    if not isinstance(name, str) or not name:
        raise RuntimeError("GitHub workflow job has no name")
    labels = job_labels(job, name)
    matches_label = any(
        label.startswith(worker_label_prefix(run_id, run_attempt)) for label in labels
    )
    if not matches_label:
        return False
    if job.get("run_attempt") != run_attempt:
        raise RuntimeError(
            f"worker {name!r} has run_attempt={job.get('run_attempt')!r}; "
            f"expected {run_attempt}"
        )
    return True


def worker_timing(
    job: JsonObject,
    queue_budget: timedelta,
) -> WorkerTiming:
    name = job.get("name")
    if not isinstance(name, str) or not name:
        raise RuntimeError("GitHub workflow job has no name")
    status = job.get("status")
    if not isinstance(status, str) or not status:
        raise RuntimeError(f"GitHub job {name!r} has no status")
    created_at = parse_timestamp(job.get("created_at"), f"{name} created_at")
    raw_started_at = job.get("started_at")
    started_at = (
        None
        if raw_started_at is None
        else parse_timestamp(raw_started_at, f"{name} started_at")
    )
    return WorkerTiming(
        name=name,
        status=status,
        created_at=created_at,
        started_at=started_at,
        queue_deadline=created_at + queue_budget,
    )


def current_workers(
    jobs: Sequence[JsonObject],
    run_id: int,
    run_attempt: int,
    queue_budget: timedelta,
) -> list[WorkerTiming]:
    return [
        worker_timing(job, queue_budget)
        for job in jobs
        if is_current_worker(job, run_id, run_attempt)
    ]


def verify_required_gate(
    client: GitHubClient,
    run_id: int,
    run_attempt: int,
    target_name: str,
    queue_budget: timedelta,
) -> None:
    attempt = workflow_attempt(client, run_id, run_attempt)
    attempt_started_at = parse_timestamp(
        attempt.get("run_started_at"), "run_started_at"
    )
    target = target_job(client.workflow_jobs(run_id, run_attempt), target_name)
    if target.get("run_attempt") != run_attempt:
        raise RuntimeError(
            f"required target {target_name!r} belongs to attempt "
            f"{target.get('run_attempt')!r}, expected {run_attempt}"
        )
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

    queue_deadline: datetime | None = None
    if is_current_worker(target, run_id, run_attempt):
        timing = worker_timing(target, queue_budget)
        queue_deadline = timing.queue_deadline
        if not timing.started_on_time:
            raise RuntimeError(
                f"required worker {target_name!r} started at "
                f"{target_started_at.isoformat()}, after "
                f"{timing.queue_deadline.isoformat()}"
            )
    print(
        json.dumps(
            {
                "queue_deadline": (
                    None if queue_deadline is None else queue_deadline.isoformat()
                ),
                "started_at": target_started_at.isoformat(),
                "target": target_name,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
    )


def cancel_stale_workers(
    client: GitHubClient,
    run_id: int,
    run_attempt: int,
    queue_budget: timedelta,
    *,
    now: Callable[[], datetime] = lambda: datetime.now(UTC),
    sleep: Callable[[float], None] = time.sleep,
) -> bool:
    while True:
        attempt = workflow_attempt(client, run_id, run_attempt)
        status = attempt.get("status")
        if not isinstance(status, str):
            raise RuntimeError("GitHub API workflow attempt has no status")
        if status == "completed":
            print("workflow attempt completed before queue cancellation")
            return False

        workers = current_workers(
            client.workflow_jobs(run_id, run_attempt),
            run_id,
            run_attempt,
            queue_budget,
        )
        timely = [worker for worker in workers if worker.started_on_time]
        if timely:
            print(
                json.dumps(
                    {
                        "queue_controller": "released",
                        "timely_workers": [worker.name for worker in timely],
                    },
                    separators=(",", ":"),
                    sort_keys=True,
                )
            )
            return False

        outstanding = [worker for worker in workers if worker.outstanding]
        if workers and not outstanding:
            print("all current-attempt workers completed before queue cancellation")
            return False

        checked_at = now()
        fresh = [
            worker
            for worker in outstanding
            if worker.started_at is None and checked_at <= worker.queue_deadline
        ]
        if not workers or fresh:
            if fresh:
                remaining = min(
                    (worker.queue_deadline - checked_at).total_seconds()
                    for worker in fresh
                )
                sleep(min(2, max(0.001, remaining)))
            else:
                sleep(2)
            continue

        stale_names = [worker.name for worker in outstanding]
        print(
            "::error title=CI worker queue deadline exceeded::"
            f"cancelling run {run_id} attempt {run_attempt}; "
            f"stale workers: {', '.join(stale_names)}"
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
    queue_budget = timedelta(seconds=POLICY.queue_start_seconds)
    if mode == "gate":
        target_name = os.environ["CI_BUDGET_TARGET_JOB_NAME"]
        if not target_name:
            raise ValueError("target-job-name must not be empty")
        verify_required_gate(
            client,
            run_id,
            run_attempt,
            target_name,
            queue_budget,
        )
        return 0
    if mode == "cancel":
        cancel_stale_workers(
            client,
            run_id,
            run_attempt,
            queue_budget,
        )
        return 0
    raise ValueError(f"unknown deadline mode {mode!r}")


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-deadline::{error}", file=sys.stderr)
        sys.exit(1)
