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
        cancel_error: RuntimeError | None = None,
        complete_on_cancel: bool = True,
        runs: list[dict[str, Any]] | None = None,
        snapshots: list[ci_deadline.BudgetSnapshot | None] | None = None,
    ) -> None:
        self.attempts = attempts
        self.runs = [run.copy() for run in (runs or [attempts[0]])]
        self.jobs = jobs or []
        self.cancel_error = cancel_error
        self.complete_on_cancel = complete_on_cancel
        self.snapshots = (
            snapshots
            if snapshots is not None
            else [ci_deadline.BudgetSnapshot(big_change=False)]
        )
        self.cancelled: list[int] = []
        self.request_timeouts: list[float] = []

    def workflow_run(
        self, run_id: int, *, timeout_seconds: float = 30.0
    ) -> dict[str, Any]:
        assert run_id == 12
        self.request_timeouts.append(timeout_seconds)
        if len(self.runs) > 1:
            return self.runs.pop(0)
        return self.runs[0]

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
        self,
        run_id: int,
        run_attempt: int,
        *,
        request_timeout: Callable[[], float] = lambda: 30.0,
    ) -> ci_deadline.BudgetSnapshot | None:
        assert run_id == 12
        assert run_attempt == 2
        self.request_timeouts.append(request_timeout())
        if not self.snapshots:
            return None
        return self.snapshots.pop(0)

    def force_cancel_workflow_run(
        self, run_id: int, *, timeout_seconds: float = 30.0
    ) -> None:
        if self.cancel_error:
            raise self.cancel_error
        self.request_timeouts.append(timeout_seconds)
        self.cancelled.append(run_id)
        if self.complete_on_cancel:
            completed = self.runs[-1].copy()
            completed["status"] = "completed"
            self.runs = [completed]


class FakeClock:
    def __init__(self, current: datetime) -> None:
        self.current = current
        self.sleeps: list[float] = []

    def now(self) -> datetime:
        return self.current

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        self.current += timedelta(seconds=seconds)


class TimingOutSnapshotClient(FakeClient):
    def __init__(self, attempts: list[dict[str, Any]], clock: FakeClock) -> None:
        super().__init__(attempts)
        self.clock = clock

    def ci_budget_snapshot(
        self,
        run_id: int,
        run_attempt: int,
        *,
        request_timeout: Callable[[], float] = lambda: 30.0,
    ) -> ci_deadline.BudgetSnapshot | None:
        assert run_id == 12
        assert run_attempt == 2
        timeout_seconds = request_timeout()
        self.request_timeouts.append(timeout_seconds)
        self.clock.sleep(timeout_seconds)
        raise ci_deadline.GitHubTransportError("snapshot request timed out")


class FirstForceTimeoutClient(FakeClient):
    def __init__(
        self,
        attempts: list[dict[str, Any]],
        clock: FakeClock,
    ) -> None:
        super().__init__(attempts)
        self.clock = clock
        self.force_attempts = 0

    def force_cancel_workflow_run(
        self, run_id: int, *, timeout_seconds: float = 30.0
    ) -> None:
        self.force_attempts += 1
        if self.force_attempts == 1:
            self.request_timeouts.append(timeout_seconds)
            self.clock.sleep(timeout_seconds)
            raise ci_deadline.GitHubTransportError("force cancellation timed out")
        super().force_cancel_workflow_run(run_id, timeout_seconds=timeout_seconds)


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
                    created_at="2026-07-15T10:00:00Z",
                    run_started_at="2026-07-15T10:00:00Z",
                )
            ],
            [target(completed_at="2026-07-15T10:00:01Z")],
            runs=[attempt(created_at="2026-07-15T09:55:00Z")],
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
    def test_deadline_crossed_between_checks_continues_cancellation(self) -> None:
        deadline = datetime(2026, 7, 15, 10, 5, tzinfo=UTC)
        instants = iter([deadline - timedelta(microseconds=1), deadline])
        client = FakeClient([attempt()])

        error = runtime_error(
            lambda: ci_deadline.cancel_and_wait_for_terminal(
                client,
                12,
                deadline,
                now=lambda: next(instants, deadline),
                sleep=lambda _: self.fail("terminal fake must not sleep"),
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.cancelled == [12]

    def test_stale_retry_is_cancelled_but_cannot_claim_timely_termination(
        self,
    ) -> None:
        client = FakeClient(
            [
                attempt(
                    created_at="2026-07-15T09:00:00Z",
                    run_started_at="2026-07-15T10:00:00Z",
                )
            ]
        )

        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                client,
                12,
                2,
                timedelta(minutes=5),
                now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
                sleep=lambda _: self.fail(
                    "stale retry must not receive a fresh budget"
                ),
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.cancelled == [12]

    def test_controller_exits_when_attempt_already_completed(self) -> None:
        client = FakeClient([attempt(status="completed")], snapshots=[None])

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            sleep=lambda _: self.fail("completed attempt must not sleep"),
        )

        assert not cancelled
        assert not client.cancelled

    def test_fork_pull_request_uses_source_snapshot_when_payload_is_empty(self) -> None:
        fork_attempt = attempt()
        fork_attempt["pull_requests"] = []
        completed = attempt(status="completed")
        completed["pull_requests"] = []
        client = FakeClient(
            [fork_attempt, completed],
            runs=[fork_attempt, completed],
            snapshots=[ci_deadline.BudgetSnapshot(big_change=False)],
        )
        clock = FakeClock(datetime(2026, 7, 15, 10, 0, tzinfo=UTC))

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=clock.now,
            sleep=clock.sleep,
        )

        assert cancelled
        assert clock.sleeps == [240]
        assert client.cancelled == [12]
        assert client.request_timeouts[-2:] == [10, 10]

    def test_missing_source_snapshot_cancels_at_deadline(self) -> None:
        client = FakeClient([attempt(), attempt()], snapshots=[None])

        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                client,
                12,
                2,
                timedelta(minutes=5),
                now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
                sleep=lambda _: self.fail("elapsed deadline must not sleep"),
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.cancelled == [12]

    def test_extended_snapshot_observed_after_cancellation_start_is_too_late(
        self,
    ) -> None:
        client = FakeClient(
            [attempt()],
            snapshots=[ci_deadline.BudgetSnapshot(big_change=True)],
        )
        clock = FakeClock(datetime(2026, 7, 15, 10, 4, 1, tzinfo=UTC))

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=clock.now,
            sleep=clock.sleep,
        )

        assert cancelled
        assert client.cancelled == [12]
        assert client.request_timeouts == [10, 10, 10]

    def test_main_push_waits_for_classified_snapshot(self) -> None:
        client = FakeClient(
            [attempt(event="push"), attempt(status="completed", event="push")],
            runs=[attempt(event="push"), attempt(status="completed", event="push")],
            snapshots=[None, ci_deadline.BudgetSnapshot(big_change=False)],
        )
        clock = FakeClock(datetime(2026, 7, 15, 10, 0, tzinfo=UTC))

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=clock.now,
            sleep=clock.sleep,
        )

        assert cancelled
        assert clock.sleeps == [2, 238]
        assert client.cancelled == [12]

    def test_merge_group_waits_for_classified_snapshot(self) -> None:
        client = FakeClient(
            [
                attempt(event="merge_group"),
                attempt(status="completed", event="merge_group"),
            ],
            runs=[
                attempt(event="merge_group"),
                attempt(status="completed", event="merge_group"),
            ],
            snapshots=[None, ci_deadline.BudgetSnapshot(big_change=False)],
        )
        clock = FakeClock(datetime(2026, 7, 15, 10, 0, tzinfo=UTC))

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=clock.now,
            sleep=clock.sleep,
        )

        assert cancelled
        assert clock.sleeps == [2, 238]
        assert client.cancelled == [12]

    def test_queue_time_counts_toward_cancellation(self) -> None:
        client = FakeClient([attempt(), attempt()])

        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                client,
                12,
                2,
                timedelta(minutes=5),
                now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
                sleep=lambda _: self.fail("elapsed deadline must not sleep"),
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.cancelled == [12]

    def test_snapshot_timeout_falls_through_to_force_cancellation(self) -> None:
        clock = FakeClock(datetime(2026, 7, 15, 10, 3, 55, tzinfo=UTC))
        client = TimingOutSnapshotClient([attempt()], clock)

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=clock.now,
            sleep=clock.sleep,
        )

        assert cancelled
        assert client.cancelled == [12]
        assert client.request_timeouts == [10, 5, 10, 10]
        assert clock.current == datetime(2026, 7, 15, 10, 4, tzinfo=UTC)

    def test_deadline_race_still_requests_idempotent_cancellation(self) -> None:
        client = FakeClient(
            [attempt(), attempt(status="completed")],
            runs=[attempt(), attempt(status="completed")],
        )

        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                client,
                12,
                2,
                timedelta(minutes=5),
                now=lambda: datetime(2026, 7, 15, 10, 5, tzinfo=UTC),
                sleep=lambda _: self.fail("elapsed deadline must not sleep"),
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.cancelled == [12]

    def test_active_run_must_be_confirmed_terminal_by_the_absolute_deadline(
        self,
    ) -> None:
        clock = FakeClock(datetime(2026, 7, 15, 10, 4, 50, tzinfo=UTC))
        client = FirstForceTimeoutClient([attempt()], clock)

        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                client,
                12,
                2,
                timedelta(minutes=5),
                now=clock.now,
                sleep=clock.sleep,
            )
        )

        assert "first confirmed terminal" in str(error)
        assert client.force_attempts == 2
        assert client.cancelled == [12]
        assert clock.current == datetime(2026, 7, 15, 10, 5, tzinfo=UTC)
        assert all(timeout <= 10 for timeout in client.request_timeouts[1:])

    def test_source_snapshot_freezes_big_change_exemption(self) -> None:
        client = FakeClient(
            [attempt()],
            snapshots=[ci_deadline.BudgetSnapshot(big_change=True)],
        )

        cancelled = ci_deadline.cancel_at_deadline(
            client,
            12,
            2,
            timedelta(minutes=5),
            now=lambda: datetime(2026, 7, 15, 10, 0, tzinfo=UTC),
            sleep=lambda _: self.fail("extended snapshot must not sleep"),
        )

        assert not cancelled

    def test_denied_cancellation_cannot_make_late_target_green(self) -> None:
        denial = RuntimeError("HTTP 403: Resource not accessible by integration")
        controller = FakeClient([attempt(), attempt()], cancel_error=denial)
        error = runtime_error(
            lambda: ci_deadline.cancel_at_deadline(
                controller,
                12,
                2,
                timedelta(minutes=5),
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
