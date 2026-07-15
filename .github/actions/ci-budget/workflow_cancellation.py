#!/usr/bin/env python3
"""Cancel a GitHub workflow through one typed, audited boundary."""

from __future__ import annotations

import json
import os
import sys
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from enum import StrEnum
from pathlib import Path
from typing import Any, Protocol

JsonObject = dict[str, Any]


class CancellationReasonCode(StrEnum):
    CI_TOTAL_DEADLINE_EXCEEDED = "ci_total_deadline_exceeded"
    CACHE_PUSH_ZOMBIE = "cache_push_zombie"
    CACHE_PUSH_MATERIALIZATION_STALL = "cache_push_materialization_stall"


class CancellationSourceKind(StrEnum):
    CI_DEADLINE_CONTROLLER = "ci_deadline_controller"
    CACHE_PUSH_WATCHDOG = "cache_push_watchdog"


class CancellationOutcome(StrEnum):
    REQUESTED = "requested"
    ACCEPTED = "accepted"
    REJECTED = "rejected"


ALLOWED_REASONS = {
    CancellationSourceKind.CI_DEADLINE_CONTROLLER: frozenset(
        {CancellationReasonCode.CI_TOTAL_DEADLINE_EXCEEDED}
    ),
    CancellationSourceKind.CACHE_PUSH_WATCHDOG: frozenset(
        {
            CancellationReasonCode.CACHE_PUSH_MATERIALIZATION_STALL,
            CancellationReasonCode.CACHE_PUSH_ZOMBIE,
        }
    ),
}


class Request(Protocol):
    def __call__(
        self,
        method: str,
        path: str,
        body: JsonObject | None = None,
        query: Mapping[str, int | str] | None = None,
    ) -> tuple[Any, Mapping[str, str]]: ...


class Canceller(Protocol):
    def cancel(self, cancellation: WorkflowCancellation) -> Path: ...


def nonempty(value: str, name: str) -> str:
    if not value.strip():
        raise ValueError(f"{name} must not be empty")
    return value


def positive(value: int, name: str) -> int:
    if value <= 0:
        raise ValueError(f"{name} must be positive")
    return value


def repository_name(value: str, name: str) -> str:
    owner, separator, repository = value.partition("/")
    if not separator or not owner or not repository or "/" in repository:
        raise ValueError(f"{name} must have owner/name form")
    return value


@dataclass(frozen=True)
class CancellationReason:
    code: CancellationReasonCode
    detail: str

    def __post_init__(self) -> None:
        if not isinstance(self.code, CancellationReasonCode):
            raise ValueError("cancellation reason code must be typed")
        nonempty(self.detail, "cancellation reason detail")


@dataclass(frozen=True)
class CancellationSource:
    kind: CancellationSourceKind
    actor: str
    repository: str
    run_id: int
    run_attempt: int
    workflow_ref: str
    job: str

    def __post_init__(self) -> None:
        if not isinstance(self.kind, CancellationSourceKind):
            raise ValueError("cancellation source kind must be typed")
        nonempty(self.actor, "cancellation actor")
        repository_name(self.repository, "cancellation source repository")
        positive(self.run_id, "cancellation source run ID")
        positive(self.run_attempt, "cancellation source run attempt")
        nonempty(self.workflow_ref, "cancellation source workflow ref")
        nonempty(self.job, "cancellation source job")


@dataclass(frozen=True)
class WorkflowCancellation:
    repository: str
    run_id: int
    run_attempt: int
    reason: CancellationReason
    source: CancellationSource

    def __post_init__(self) -> None:
        repository_name(self.repository, "cancellation target repository")
        positive(self.run_id, "cancellation target run ID")
        positive(self.run_attempt, "cancellation target run attempt")
        if self.repository != self.source.repository:
            raise ValueError(
                "workflow cancellation source and target repositories must match"
            )
        allowed = ALLOWED_REASONS[self.source.kind]
        if self.reason.code not in allowed:
            raise ValueError(
                f"cancellation source {self.source.kind.value!r} cannot use "
                f"reason {self.reason.code.value!r}"
            )


def source_from_environment(
    kind: CancellationSourceKind,
    environment: Mapping[str, str],
) -> CancellationSource:
    return CancellationSource(
        kind=kind,
        actor=environment["GITHUB_ACTOR"],
        repository=environment["GITHUB_REPOSITORY"],
        run_id=int(environment["GITHUB_RUN_ID"]),
        run_attempt=int(environment["GITHUB_RUN_ATTEMPT"]),
        workflow_ref=environment["GITHUB_WORKFLOW_REF"],
        job=environment["GITHUB_JOB"],
    )


def cancellation_record(
    cancellation: WorkflowCancellation,
    outcome: CancellationOutcome,
    recorded_at: datetime,
    *,
    error: str | None = None,
) -> JsonObject:
    record: JsonObject = {
        "actor": cancellation.source.actor,
        "outcome": outcome.value,
        "reason": {
            "code": cancellation.reason.code.value,
            "detail": cancellation.reason.detail,
        },
        "recorded_at": recorded_at.astimezone(UTC).isoformat().replace("+00:00", "Z"),
        "schema_version": 1,
        "source": {
            "job": cancellation.source.job,
            "kind": cancellation.source.kind.value,
            "repository": cancellation.source.repository,
            "run_attempt": cancellation.source.run_attempt,
            "run_id": cancellation.source.run_id,
            "workflow_ref": cancellation.source.workflow_ref,
        },
        "target": {
            "repository": cancellation.repository,
            "run_attempt": cancellation.run_attempt,
            "run_id": cancellation.run_id,
        },
    }
    if error is not None:
        record["error"] = error
    return record


class WorkflowCanceller:
    def __init__(
        self,
        request: Request,
        repository: str,
        record_directory: Path,
        summary_path: Path,
        *,
        now: Callable[[], datetime] = lambda: datetime.now(UTC),
    ) -> None:
        self._request = request
        self._repository = repository_name(repository, "GitHub client repository")
        self._record_directory = record_directory
        self._summary_path = summary_path
        self._now = now

    def _path(self, cancellation: WorkflowCancellation) -> Path:
        repository = cancellation.repository.replace("/", "__")
        return self._record_directory / (
            f"{repository}__{cancellation.run_id}__attempt_"
            f"{cancellation.run_attempt}__source_{cancellation.source.run_id}_"
            f"attempt_{cancellation.source.run_attempt}.json"
        )

    def _write(self, path: Path, record: JsonObject) -> None:
        self._record_directory.mkdir(parents=True, exist_ok=True)
        encoded = json.dumps(record, indent=2, sort_keys=True) + "\n"
        temporary = path.with_suffix(".tmp")
        temporary.write_text(encoded, encoding="utf-8")
        temporary.replace(path)
        compact = json.dumps(record, separators=(",", ":"), sort_keys=True)
        print(f"workflow-cancellation {compact}")
        with self._summary_path.open("a", encoding="utf-8") as summary:
            summary.write(f"`workflow-cancellation {compact}`\n\n")

    def cancel(self, cancellation: WorkflowCancellation) -> Path:
        if cancellation.repository != self._repository:
            raise ValueError(
                "workflow cancellation target does not match the GitHub client"
            )
        path = self._path(cancellation)
        self._write(
            path,
            cancellation_record(
                cancellation,
                CancellationOutcome.REQUESTED,
                self._now(),
            ),
        )
        try:
            self._request("POST", f"actions/runs/{cancellation.run_id}/cancel")
        except Exception as error:
            self._write(
                path,
                cancellation_record(
                    cancellation,
                    CancellationOutcome.REJECTED,
                    self._now(),
                    error=f"{type(error).__name__}: {error}",
                ),
            )
            raise
        self._write(
            path,
            cancellation_record(
                cancellation,
                CancellationOutcome.ACCEPTED,
                self._now(),
            ),
        )
        return path


def main() -> int:
    from ci_budget import GitHubClient

    environment = os.environ
    source = source_from_environment(
        CancellationSourceKind(environment["WORKFLOW_CANCELLATION_SOURCE"]),
        environment,
    )
    cancellation = WorkflowCancellation(
        repository=environment["WORKFLOW_CANCELLATION_TARGET_REPOSITORY"],
        run_id=int(environment["WORKFLOW_CANCELLATION_TARGET_RUN_ID"]),
        run_attempt=int(environment["WORKFLOW_CANCELLATION_TARGET_RUN_ATTEMPT"]),
        reason=CancellationReason(
            code=CancellationReasonCode(
                environment["WORKFLOW_CANCELLATION_REASON_CODE"]
            ),
            detail=environment["WORKFLOW_CANCELLATION_REASON_DETAIL"],
        ),
        source=source,
    )
    client = GitHubClient(
        cancellation.repository,
        environment["WORKFLOW_CANCELLATION_TOKEN"],
    )
    WorkflowCanceller(
        client.request,
        cancellation.repository,
        Path(environment["WORKFLOW_CANCELLATION_RECORD_DIRECTORY"]),
        Path(environment["GITHUB_STEP_SUMMARY"]),
    ).cancel(cancellation)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=workflow-cancellation::{error}", file=sys.stderr)
        sys.exit(1)
