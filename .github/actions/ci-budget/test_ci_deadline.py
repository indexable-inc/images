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
        jobs: list[dict[str, Any]] | None = None,
        *,
        snapshots: list[ci_deadline.BudgetSnapshot | None] | None = None,
    ) -> None:
        self.attempts = attempts
        self.jobs = jobs or []
        self.snapshots = (
            snapshots
            if snapshots is not None
            else [ci_deadline.BudgetSnapshot(big_change=False)]
        )

    def workflow_attempt(self, run_id: int, run_attempt: int) -> dict[str, Any]:
        assert run_id == 12
        assert run_attempt == 2
        if len(self.attempts) > 1:
            return self.attempts.pop(0)
        return self.attempts[0]

    def workflow_jobs(self, run_id: int, run_attempt: int) -> list[dict[str, Any]]:
        assert run_id == 12
        assert run_attempt == 2
        return self.jobs

    def ci_budget_snapshot(
        self, run_id: int, run_attempt: int
    ) -> ci_deadline.BudgetSnapshot | None:
        assert run_id == 12
        assert run_attempt == 2
        if not self.snapshots:
            return None
        return self.snapshots.pop(0)


class FakeCanceller:
    def __init__(self, error: RuntimeError | None = None) -> None:
        self.error = error
        self.cancellations: list[ci_deadline.WorkflowCancellation] = []

    def cancel(self, cancellation: ci_deadline.WorkflowCancellation) -> Path:
        if self.error is not None:
            raise self.error
        self.cancellations.append(cancellation)
        return Path("cancellation.json")


SOURCE = ci_deadline.CancellationSource(
    kind=ci_deadline.CancellationSourceKind.CI_DEADLINE_CONTROLLER,
    actor="github-actions[bot]",
    repository="indexable-inc/index",
    run_id=99,
    run_attempt=1,
    workflow_ref="indexable-inc/index/.github/workflows/ci-deadline-controller.yml@main",
    job="deadline",
)
REPOSITORY = "indexable-inc/index"


def attempt(
    *,
    status: str = "in_progress",
    event: str = "pull_request",
    created_at: str = "2026-07-15T10:00:00Z",
    run_started_at: str = "2026-07-15T10:00:00Z",
) -> dict[str, Any]:
    return {
        "created_at": created_at,
        "event": event,
        "head_sha": "a" * 40,
        "pull_requests": [] if event == "push" else [{"number": 42}],
        "run_started_at": run_started_at,
        "status": status,
    }


def target(
    *,
    conclusion: str = "success",
    started_at: str = "2026-07-15T10:00:01Z",
    completed_at: str = "2026-07-15T10:04:59Z",
) -> dict[str, Any]:
    return {
        "name": "flake-build",
        "status": "completed",
        "conclusion": conclusion,
        "started_at": started_at,
        "completed_at": completed_at,
    }


class RequiredGateTests(unittest.TestCase):
    def test_attempt_two_target_rerun_before_deadline_passes(self) -> None:
        client = FakeClient([attempt()], [target()])

        ci_deadline.verify_required_gate(
            client,
            12,
            2,
            "flake-build",
            timedelta(minutes=5),
            big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 4, 59, tzinfo=UTC),
        )

    def test_terminal_gate_cannot_turn_green_after_the_deadline(self) -> None:
        client = FakeClient([attempt()], [target()])

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
                now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
            )
        )

        assert "required terminal gate ran" in str(error)

    def test_retry_does_not_reset_the_workflow_creation_deadline(self) -> None:
        client = FakeClient(
            [
                attempt(
                    created_at="2026-07-15T09:55:00Z",
                    run_started_at="2026-07-15T10:00:00Z",
                )
            ],
            [target(completed_at="2026-07-15T10:00:01Z")],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
            )
        )

        assert "after 2026-07-15T10:00:00" in str(error)

    def test_failed_target_fails_required_gate(self) -> None:
        client = FakeClient([attempt()], [target(conclusion="failure")])

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
            )
        )

        assert "conclusion='failure'" in str(error)

    def test_cancelled_skipped_and_pending_targets_fail(self) -> None:
        cases = (
            ("completed", "cancelled"),
            ("completed", "skipped"),
            ("in_progress", None),
        )
        for status, conclusion in cases:
            with self.subTest(status=status, conclusion=conclusion):
                job = target()
                job["status"] = status
                job["conclusion"] = conclusion
                client = FakeClient([attempt()], [job])

                error = runtime_error(
                    lambda client=client: ci_deadline.verify_required_gate(
                        client,
                        12,
                        2,
                        "flake-build",
                        timedelta(minutes=5),
                        big_change=False,
                    )
                )

                assert "ended with status" in str(error)

    def test_missing_and_duplicate_targets_fail(self) -> None:
        for jobs, count in (([], 0), ([target(), target()], 2)):
            with self.subTest(count=count):
                client = FakeClient([attempt()], jobs)

                error = runtime_error(
                    lambda client=client: ci_deadline.verify_required_gate(
                        client,
                        12,
                        2,
                        "flake-build",
                        timedelta(minutes=5),
                        big_change=False,
                    )
                )

                assert f"has {count} jobs" in str(error)

    def test_success_at_five_minutes_one_second_fails(self) -> None:
        client = FakeClient(
            [attempt()],
            [target(completed_at="2026-07-15T10:05:01Z")],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
            )
        )

        assert "after 2026-07-15T10:05:00" in str(error)

    def test_attempt_two_rejects_target_reused_from_attempt_one(self) -> None:
        client = FakeClient(
            [attempt()],
            [
                target(
                    started_at="2026-07-15T09:50:00Z",
                    completed_at="2026-07-15T09:54:00Z",
                )
            ],
        )

        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                client,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
            )
        )

        assert "reused from an earlier attempt" in str(error)

    def test_big_change_still_requires_success_but_not_deadline(self) -> None:
        client = FakeClient(
            [attempt()],
            [target(completed_at="2026-07-15T11:00:00Z")],
        )

        ci_deadline.verify_required_gate(
            client,
            12,
            2,
            "flake-build",
            timedelta(minutes=5),
            big_change=True,
        )


class CancellationControllerTests(unittest.TestCase):
    def test_stale_retry_is_cancelled_without_a_fresh_budget(self) -> None:
        client = FakeClient(
            [
                attempt(
                    created_at="2026-07-15T09:00:00Z",
                    run_started_at="2026-07-15T10:00:00Z",
                )
            ]
        )
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
            sleep=lambda _: self.fail("stale retry must not receive a fresh budget"),
        )

        assert cancelled
        assert [item.run_id for item in canceller.cancellations] == [12]
        assert canceller.cancellations[0].reason.code == (
            ci_deadline.CancellationReasonCode.CI_TOTAL_DEADLINE_EXCEEDED
        )
        assert "300 second total budget" in (canceller.cancellations[0].reason.detail)

    def test_controller_exits_when_attempt_already_completed(self) -> None:
        client = FakeClient([attempt(status="completed")], snapshots=[None])
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            sleep=lambda _: self.fail("completed attempt must not sleep"),
        )

        assert not cancelled
        assert not canceller.cancellations

    def test_fork_pull_request_uses_source_snapshot_when_payload_is_empty(self) -> None:
        fork_attempt = attempt()
        fork_attempt["pull_requests"] = []
        completed = attempt(status="completed")
        completed["pull_requests"] = []
        client = FakeClient(
            [fork_attempt, completed],
            snapshots=[ci_deadline.BudgetSnapshot(big_change=False)],
        )
        canceller = FakeCanceller()
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
            sleep=sleeps.append,
        )

        assert not cancelled
        assert sleeps == [300]

    def test_missing_source_snapshot_cancels_at_deadline(self) -> None:
        client = FakeClient([attempt(), attempt()], snapshots=[None])
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("elapsed deadline must not sleep"),
        )

        assert cancelled
        assert [item.run_id for item in canceller.cancellations] == [12]

    def test_main_push_waits_for_classified_snapshot(self) -> None:
        client = FakeClient(
            [attempt(event="push"), attempt(status="completed", event="push")],
            snapshots=[None, ci_deadline.BudgetSnapshot(big_change=False)],
        )
        canceller = FakeCanceller()
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
            sleep=sleeps.append,
        )

        assert not cancelled
        assert sleeps == [2, 300]

    def test_merge_group_waits_for_classified_snapshot(self) -> None:
        client = FakeClient(
            [
                attempt(event="merge_group"),
                attempt(status="completed", event="merge_group"),
            ],
            snapshots=[None, ci_deadline.BudgetSnapshot(big_change=False)],
        )
        canceller = FakeCanceller()
        sleeps: list[float] = []

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
            sleep=sleeps.append,
        )

        assert not cancelled
        assert sleeps == [2, 300]

    def test_queue_time_counts_toward_cancellation(self) -> None:
        client = FakeClient([attempt(), attempt()])
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("elapsed deadline must not sleep"),
        )

        assert cancelled
        assert [item.run_id for item in canceller.cancellations] == [12]

    def test_completed_attempt_is_not_cancelled(self) -> None:
        client = FakeClient([attempt(), attempt(status="completed")])
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            now=lambda: datetime(2026, 7, 15, 10, 5, tzinfo=UTC),
            sleep=lambda _: self.fail("elapsed deadline must not sleep"),
        )

        assert not cancelled
        assert not canceller.cancellations

    def test_big_change_is_exempt(self) -> None:
        client = FakeClient([attempt()])
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=True,
            sleep=lambda _: self.fail("big change must not sleep"),
        )

        assert not cancelled

    def test_source_snapshot_freezes_big_change_exemption(self) -> None:
        client = FakeClient(
            [attempt()],
            snapshots=[ci_deadline.BudgetSnapshot(big_change=True)],
        )
        canceller = FakeCanceller()

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            canceller,
            REPOSITORY,
            12,
            2,
            timedelta(minutes=5),
            source=SOURCE,
            force_big_change=False,
            sleep=lambda _: self.fail("extended snapshot must not sleep"),
        )

        assert not cancelled

    def test_denied_cancellation_cannot_make_late_target_green(self) -> None:
        denial = RuntimeError("HTTP 403: Resource not accessible by integration")
        controller = FakeClient([attempt(), attempt()])
        canceller = FakeCanceller(denial)
        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                controller,
                canceller,
                REPOSITORY,
                12,
                2,
                timedelta(minutes=5),
                source=SOURCE,
                force_big_change=False,
                now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
                sleep=lambda _: self.fail("elapsed deadline must not sleep"),
            )
        )

        assert "HTTP 403" in str(error)

        gate = FakeClient(
            [attempt()],
            [target(completed_at="2026-07-15T10:05:01Z")],
        )
        error = runtime_error(
            lambda: ci_deadline.verify_required_gate(
                gate,
                12,
                2,
                "flake-build",
                timedelta(minutes=5),
                big_change=False,
            )
        )

        assert "after 2026-07-15T10:05:00" in str(error)


if __name__ == "__main__":
    unittest.main()
