from __future__ import annotations

import importlib.util
import sys
import unittest
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


class FakeClient:
    def __init__(self, started_at: str, job_pages: list[list[dict[str, Any]]]) -> None:
        self.started_at = started_at
        self.job_pages = job_pages
        self.cancelled: list[int] = []

    def workflow_attempt(self, run_id: int, run_attempt: int) -> dict[str, Any]:
        assert run_id == 12
        assert run_attempt == 3
        return {"run_started_at": self.started_at}

    def workflow_jobs(self, run_id: int, run_attempt: int) -> list[dict[str, Any]]:
        assert run_id == 12
        assert run_attempt == 3
        return self.job_pages.pop(0)

    def cancel_workflow_run(self, run_id: int) -> None:
        self.cancelled.append(run_id)


class DeadlineTests(unittest.TestCase):
    def test_queue_time_counts_toward_deadline(self) -> None:
        client = FakeClient(
            "2026-07-15T10:00:00Z",
            [[{"name": "flake-check", "status": "queued"}]],
        )

        cancelled = ci_deadline.enforce(
            client,
            12,
            3,
            ["flake-check"],
            timedelta(minutes=5),
            now=lambda: datetime(2026, 7, 15, 10, 5, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("deadline must not sleep"),
        )

        assert cancelled
        assert client.cancelled == [12]

    def test_completed_target_stops_monitor(self) -> None:
        client = FakeClient(
            "2026-07-15T10:00:00Z",
            [[{"name": "flake-check", "status": "completed"}]],
        )

        cancelled = ci_deadline.enforce(
            client,
            12,
            3,
            ["flake-check"],
            timedelta(minutes=5),
            now=lambda: datetime(2026, 7, 15, 10, 1, tzinfo=UTC),
            sleep=lambda _: self.fail("completed target must not sleep"),
        )

        assert not cancelled
        assert not client.cancelled

    def test_missing_target_is_cancelled_at_deadline(self) -> None:
        client = FakeClient("2026-07-15T10:00:00Z", [[]])

        cancelled = ci_deadline.enforce(
            client,
            12,
            3,
            ["flake-check"],
            timedelta(minutes=5),
            now=lambda: datetime(2026, 7, 15, 10, 5, tzinfo=UTC),
            sleep=lambda _: self.fail("deadline must not sleep"),
        )

        assert cancelled


if __name__ == "__main__":
    unittest.main()
