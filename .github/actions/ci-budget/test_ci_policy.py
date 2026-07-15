from __future__ import annotations

import json
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

from ci_policy import (
    load_policy,
    standard_deadline,
    standard_minutes,
    worker_timeout_minutes,
)


def policy_error(path: Path) -> RuntimeError:
    try:
        load_policy(path)
    except RuntimeError as error:
        return error
    raise AssertionError("policy did not fail")


class PolicyTests(unittest.TestCase):
    def test_shared_worker_envelopes(self) -> None:
        assert standard_minutes() == 5
        assert worker_timeout_minutes(big_change=False) == 5
        assert worker_timeout_minutes(big_change=True) == 183

    def test_standard_deadline_uses_workflow_creation_not_retry_start(self) -> None:
        run = {
            "created_at": "2026-07-15T10:00:00+00:00",
            "run_started_at": "2026-07-15T11:00:00+00:00",
        }

        assert standard_deadline(run) == datetime(2026, 7, 15, 10, 5, tzinfo=UTC)

    def test_policy_rejects_unknown_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps({"standard_seconds": 300}))

            assert "policy keys" in str(policy_error(path))

    def test_policy_rejects_boolean_integer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(
                json.dumps(
                    {
                        "extended_setup_allowance_seconds": 120,
                        "extended_validation_seconds": 10_800,
                        "standard_seconds": True,
                        "termination_grace_seconds": 10,
                    }
                )
            )

            assert "positive integer" in str(policy_error(path))


if __name__ == "__main__":
    unittest.main()
