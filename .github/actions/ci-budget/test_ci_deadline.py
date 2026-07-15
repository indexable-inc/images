from __future__ import annotations

import importlib.util
import sys
import unittest
from collections.abc import Callable
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("ci_deadline.py")
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("ci_deadline", MODULE_PATH)
assert SPEC is not None
assert SPEC.loader is not None
ci_deadline = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci_deadline
SPEC.loader.exec_module(ci_deadline)

QUEUE_BUDGET = timedelta(minutes=5)


def runtime_error(call: Callable[[], object]) -> RuntimeError:
    try:
        call()
    except RuntimeError as error:
        return error
    raise AssertionError("call did not raise RuntimeError")


class FakeClient:
    def __init__(
        self,
        attempts: list[dict[str, Any]],
        job_batches: list[list[dict[str, Any]]] | None = None,
        *,
        cancel_error: RuntimeError | None = None,
    ) -> None:
        self.attempts = attempts
        self.job_batches = job_batches or [[]]
        self.cancel_error = cancel_error
        self.cancelled: list[int] = []

    def workflow_attempt(self, run_id: int, run_attempt: int) -> dict[str, Any]:
        assert run_id == 12
        assert run_attempt == 2
        if len(self.attempts) > 1:
            return self.attempts.pop(0)
        return self.attempts[0]

    def workflow_jobs(self, run_id: int, run_attempt: int) -> list[dict[str, Any]]:
        assert run_id == 12
        assert run_attempt == 2
        if len(self.job_batches) > 1:
            return self.job_batches.pop(0)
        return self.job_batches[0]

    def cancel_workflow_run(self, run_id: int) -> None:
        if self.cancel_error:
            raise self.cancel_error
        self.cancelled.append(run_id)


def attempt(
    *,
    status: str = "in_progress",
    run_attempt: int = 2,
    run_started_at: str = "2026-07-15T10:00:00Z",
) -> dict[str, Any]:
    return {
        "run_attempt": run_attempt,
        "run_started_at": run_started_at,
        "status": status,
    }


def target(
    *,
    attempt_number: int = 2,
    completed_at: str = "2026-07-15T11:00:00Z",
    conclusion: str | None = "success",
    created_at: str = "2026-07-15T10:00:00Z",
    label_attempt: int = 2,
    name: str = "flake-build",
    started_at: str | None = "2026-07-15T10:05:00Z",
    status: str = "completed",
    worker: bool = True,
) -> dict[str, Any]:
    labels = (
        [f"ix-ci-run-12-{label_attempt}-{name}"] if worker else ["ubuntu-latest"]
    )
    return {
        "completed_at": completed_at,
        "conclusion": conclusion,
        "created_at": created_at,
        "labels": labels,
        "name": name,
        "run_attempt": attempt_number,
        "started_at": started_at,
        "status": status,
    }


class RequiredGateTests(unittest.TestCase):
    def test_successful_worker_can_finish_long_after_queue_admission(self) -> None:
        client = FakeClient([attempt()], [[target()]])

        ci_deadline.verify_required_gate(
            client,
            12,
            2,
            "flake-build",
            QUEUE_BUDGET,
        )

    def test_worker_start_boundary_is_inclusive(self) -> None:
        client = FakeClient([attempt()], [[target(started_at="2026-07-15T10:05:00Z")]])

        ci_deadline.verify_required_gate(
            client,
            12,
            2,
            "flake-build",
            QUEUE_BUDGET,
        )

    def test_worker_start_after_queue_deadline_fails(self) -> None:
        client = FakeClient(
            [attempt()],
            [[target(started_at="2026-07-15T10:05:00.001Z")]],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                QUEUE_BUDGET,
            )
        )

        assert "required worker" in str(error)
        assert "after 2026-07-15T10:05:00" in str(error)

    def test_failed_cancelled_skipped_and_pending_targets_fail(self) -> None:
        cases = (
            ("completed", "failure"),
            ("completed", "cancelled"),
            ("completed", "skipped"),
            ("in_progress", None),
        )
        for status, conclusion in cases:
            with self.subTest(status=status, conclusion=conclusion):
                client = FakeClient(
                    [attempt()],
                    [[target(status=status, conclusion=conclusion)]],
                )

                error = runtime_error(
                    lambda client=client: ci_deadline.verify_required_gate(
                        client,
                        12,
                        2,
                        "flake-build",
                        QUEUE_BUDGET,
                    )
                )

                assert "ended with status" in str(error)

    def test_missing_and_duplicate_targets_fail(self) -> None:
        for jobs, count in (([], 0), ([target(), target()], 2)):
            with self.subTest(count=count):
                client = FakeClient([attempt()], [jobs])

                error = runtime_error(
                    lambda client=client: ci_deadline.verify_required_gate(
                        client,
                        12,
                        2,
                        "flake-build",
                        QUEUE_BUDGET,
                    )
                )

                assert f"has {count} jobs" in str(error)

    def test_attempt_two_rejects_target_reused_from_attempt_one(self) -> None:
        client = FakeClient(
            [attempt()],
            [[target(attempt_number=1, label_attempt=1)]],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                QUEUE_BUDGET,
            )
        )

        assert "belongs to attempt 1" in str(error)

    def test_attempt_timestamp_rejects_reused_job_even_if_attempt_field_matches(
        self,
    ) -> None:
        client = FakeClient(
            [attempt(run_started_at="2026-07-15T10:00:00Z")],
            [[target(started_at="2026-07-15T09:59:00Z")]],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                QUEUE_BUDGET,
            )
        )

        assert "reused from an earlier attempt" in str(error)

    def test_github_hosted_aggregate_has_no_self_hosted_queue_clock(self) -> None:
        client = FakeClient(
            [attempt()],
            [[target(started_at="2026-07-15T10:20:00Z", worker=False)]],
        )

        ci_deadline.verify_required_gate(
            client,
            12,
            2,
            "flake-build",
            QUEUE_BUDGET,
        )


class CancellationControllerTests(unittest.TestCase):
    def test_timely_active_worker_is_never_cancelled(self) -> None:
        client = FakeClient(
            [attempt()],
            [[target(status="in_progress", conclusion=None)]],
        )

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: datetime(2026, 7, 15, 12, 0, tzinfo=UTC),
            sleep=lambda _: self.fail("timely worker must release the controller"),
        )

        assert not cancelled
        assert not client.cancelled

    def test_timely_worker_protects_a_mixed_attempt_with_stale_sibling(self) -> None:
        jobs = [
            target(
                name="lint-build",
                started_at="2026-07-15T10:00:30Z",
                status="in_progress",
                conclusion=None,
            ),
            target(
                name="nix-build",
                started_at=None,
                status="queued",
                conclusion=None,
            ),
        ]
        client = FakeClient([attempt()], [jobs])

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: datetime(2026, 7, 15, 10, 6, tzinfo=UTC),
            sleep=lambda _: self.fail("timely worker must release the controller"),
        )

        assert not cancelled
        assert not client.cancelled

    def test_all_stale_queued_workers_cancel_the_attempt(self) -> None:
        jobs = [
            target(
                name="lint-build",
                started_at=None,
                status="queued",
                conclusion=None,
            ),
            target(
                name="nix-build",
                created_at="2026-07-15T10:01:00Z",
                started_at=None,
                status="queued",
                conclusion=None,
            ),
        ]
        client = FakeClient([attempt()], [jobs])

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: datetime(2026, 7, 15, 10, 6, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("stale workers must not sleep"),
        )

        assert cancelled
        assert client.cancelled == [12]

    def test_controller_waits_for_every_outstanding_worker_to_become_stale(
        self,
    ) -> None:
        jobs = [
            target(
                name="lint-build",
                started_at=None,
                status="queued",
                conclusion=None,
            ),
            target(
                name="nix-build",
                created_at="2026-07-15T10:02:00Z",
                started_at=None,
                status="queued",
                conclusion=None,
            ),
        ]
        client = FakeClient([attempt(), attempt()], [jobs, jobs])
        times = iter(
            [
                datetime(2026, 7, 15, 10, 6, tzinfo=UTC),
                datetime(2026, 7, 15, 10, 7, 1, tzinfo=UTC),
            ]
        )
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: next(times),
            sleep=sleeps.append,
        )

        assert cancelled
        assert sleeps == [2]
        assert client.cancelled == [12]

    def test_worker_materializing_after_dependencies_can_start_on_time(self) -> None:
        client = FakeClient(
            [attempt(), attempt()],
            [[], [target(status="in_progress", conclusion=None)]],
        )
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: datetime(2026, 7, 15, 10, 6, tzinfo=UTC),
            sleep=sleeps.append,
        )

        assert not cancelled
        assert sleeps == [2]
        assert not client.cancelled

    def test_late_active_worker_is_stale_for_controller_cleanup(self) -> None:
        client = FakeClient(
            [attempt()],
            [
                [
                    target(
                        started_at="2026-07-15T10:05:01Z",
                        status="in_progress",
                        conclusion=None,
                    )
                ]
            ],
        )

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("late active worker is already stale"),
        )

        assert cancelled
        assert client.cancelled == [12]

    def test_completed_attempt_and_completed_workers_are_not_cancelled(self) -> None:
        cases = (
            FakeClient([attempt(status="completed")]),
            FakeClient(
                [attempt()],
                [[target(started_at="2026-07-15T10:05:01Z")]],
            ),
        )
        for client in cases:
            with self.subTest():
                cancelled = ci_deadline.cancel_stale_workers(
                    client,
                    12,
                    2,
                    QUEUE_BUDGET,
                    sleep=lambda _: self.fail("terminal work must not sleep"),
                )

                assert not cancelled
                assert not client.cancelled

    def test_previous_attempt_worker_does_not_count_as_current(self) -> None:
        old = target(attempt_number=1, label_attempt=1, status="in_progress")
        client = FakeClient(
            [attempt(), attempt(status="completed")],
            [[old], [old]],
        )
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_stale_workers(
            client,
            12,
            2,
            QUEUE_BUDGET,
            sleep=sleeps.append,
        )

        assert not cancelled
        assert sleeps == [2]

    def test_inconsistent_current_worker_attempt_fails_closed(self) -> None:
        inconsistent = target(attempt_number=1, label_attempt=2, status="queued")
        client = FakeClient([attempt()], [[inconsistent]])

        error = runtime_error(
            lambda: ci_deadline.cancel_stale_workers(
                client,
                12,
                2,
                QUEUE_BUDGET,
                sleep=lambda _: self.fail("invalid worker must not sleep"),
            )
        )

        assert "run_attempt=1" in str(error)

    def test_denied_cancellation_is_loud(self) -> None:
        denial = RuntimeError("HTTP 403: Resource not accessible by integration")
        client = FakeClient(
            [attempt()],
            [[target(started_at=None, status="queued", conclusion=None)]],
            cancel_error=denial,
        )

        error = runtime_error(
            lambda: ci_deadline.cancel_stale_workers(
                client,
                12,
                2,
                QUEUE_BUDGET,
                now=lambda: datetime(2026, 7, 15, 10, 6, tzinfo=UTC),
                sleep=lambda _: self.fail("stale worker must not sleep"),
            )
        )

        assert "HTTP 403" in str(error)


if __name__ == "__main__":
    unittest.main()
