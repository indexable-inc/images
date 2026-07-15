from __future__ import annotations

import importlib.util
import json
import re
import sys
import tempfile
import unittest
from collections.abc import Callable, Mapping
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("workflow_cancellation.py")
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("workflow_cancellation", MODULE_PATH)
assert SPEC is not None
assert SPEC.loader is not None
workflow_cancellation = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = workflow_cancellation
SPEC.loader.exec_module(workflow_cancellation)

REPOSITORY = MODULE_PATH.parents[3]


def value_error(call: Callable[[], object]) -> ValueError:
    try:
        call()
    except ValueError as error:
        return error
    raise AssertionError("call did not raise ValueError")


def runtime_error(call: Callable[[], object]) -> RuntimeError:
    try:
        call()
    except RuntimeError as error:
        return error
    raise AssertionError("call did not raise RuntimeError")


class FakeRequest:
    def __init__(self, error: RuntimeError | None = None) -> None:
        self.error = error
        self.calls: list[tuple[str, str]] = []

    def __call__(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
        query: Mapping[str, int | str] | None = None,
    ) -> tuple[Any, Mapping[str, str]]:
        assert body is None
        assert query is None
        self.calls.append((method, path))
        if self.error is not None:
            raise self.error
        return None, {}


def source(
    kind: workflow_cancellation.CancellationSourceKind = (
        workflow_cancellation.CancellationSourceKind.CI_DEADLINE_CONTROLLER
    ),
) -> workflow_cancellation.CancellationSource:
    return workflow_cancellation.CancellationSource(
        kind=kind,
        actor="github-actions[bot]",
        repository="indexable-inc/index",
        run_id=99,
        run_attempt=2,
        workflow_ref=(
            "indexable-inc/index/.github/workflows/"
            "ci-deadline-controller.yml@refs/heads/main"
        ),
        job="deadline",
    )


def cancellation(
    *,
    detail: str = "ordinary CI exceeded its 300 second total budget",
) -> workflow_cancellation.WorkflowCancellation:
    return workflow_cancellation.WorkflowCancellation(
        repository="indexable-inc/ix",
        run_id=12,
        run_attempt=3,
        reason=workflow_cancellation.CancellationReason(
            code=(
                workflow_cancellation.CancellationReasonCode.CI_TOTAL_DEADLINE_EXCEEDED
            ),
            detail=detail,
        ),
        source=source(),
    )


class WorkflowCancellationTests(unittest.TestCase):
    def test_reason_must_not_be_empty(self) -> None:
        for detail in ("", "   ", "\n"):
            with self.subTest(detail=detail):
                error = value_error(lambda detail=detail: cancellation(detail=detail))
                assert "must not be empty" in str(error)

    def test_source_can_use_only_its_owned_reason_codes(self) -> None:
        error = value_error(
            lambda: workflow_cancellation.WorkflowCancellation(
                repository="indexable-inc/index",
                run_id=12,
                run_attempt=1,
                reason=workflow_cancellation.CancellationReason(
                    code=(
                        workflow_cancellation.CancellationReasonCode.CACHE_PUSH_ZOMBIE
                    ),
                    detail="wrong owner",
                ),
                source=source(),
            )
        )
        assert "cannot use reason" in str(error)

    def test_reason_code_must_be_typed(self) -> None:
        untyped_code: Any = "ci_total_deadline_exceeded"
        error = value_error(
            lambda: workflow_cancellation.CancellationReason(
                code=untyped_code,
                detail="ordinary CI exceeded its budget",
            )
        )

        assert "reason code must be typed" in str(error)

    def test_accepted_cancellation_has_a_durable_structured_record(self) -> None:
        request = FakeRequest()
        recorded_at = datetime(2026, 7, 15, 16, 7, 43, tzinfo=UTC)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            summary = root / "summary.md"
            path = workflow_cancellation.WorkflowCanceller(
                request,
                root / "records",
                summary,
                now=lambda: recorded_at,
            ).cancel(cancellation())

            record = json.loads(path.read_text())
            assert record == {
                "actor": "github-actions[bot]",
                "outcome": "accepted",
                "reason": {
                    "code": "ci_total_deadline_exceeded",
                    "detail": "ordinary CI exceeded its 300 second total budget",
                },
                "recorded_at": "2026-07-15T16:07:43Z",
                "schema_version": 1,
                "source": {
                    "job": "deadline",
                    "kind": "ci_deadline_controller",
                    "repository": "indexable-inc/index",
                    "run_attempt": 2,
                    "run_id": 99,
                    "workflow_ref": (
                        "indexable-inc/index/.github/workflows/"
                        "ci-deadline-controller.yml@refs/heads/main"
                    ),
                },
                "target": {
                    "repository": "indexable-inc/ix",
                    "run_attempt": 3,
                    "run_id": 12,
                },
            }
            assert request.calls == [("POST", "actions/runs/12/cancel")]
            summary_text = summary.read_text()
            assert '"actor":"github-actions[bot]"' in summary_text
            assert '"outcome":"requested"' in summary_text
            assert '"outcome":"accepted"' in summary_text

    def test_rejected_cancellation_preserves_the_reason_and_error(self) -> None:
        request = FakeRequest(RuntimeError("HTTP 403"))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            canceller = workflow_cancellation.WorkflowCanceller(
                request,
                root / "records",
                root / "summary.md",
            )

            error = runtime_error(lambda: canceller.cancel(cancellation()))
            assert "HTTP 403" in str(error)

            records = list((root / "records").glob("*.json"))
            assert len(records) == 1
            record = json.loads(records[0].read_text())
            assert record["outcome"] == "rejected"
            assert record["reason"]["code"] == "ci_total_deadline_exceeded"
            assert record["error"] == "RuntimeError: HTTP 403"

    def test_source_identity_comes_from_github_environment(self) -> None:
        result = workflow_cancellation.source_from_environment(
            workflow_cancellation.CancellationSourceKind.CACHE_PUSH_WATCHDOG,
            {
                "GITHUB_ACTOR": "github-actions[bot]",
                "GITHUB_REPOSITORY": "indexable-inc/index",
                "GITHUB_RUN_ID": "44",
                "GITHUB_RUN_ATTEMPT": "2",
                "GITHUB_WORKFLOW_REF": "indexable-inc/index/watchdog.yml@main",
                "GITHUB_JOB": "watch",
            },
        )

        assert result.actor == "github-actions[bot]"
        assert result.kind == (
            workflow_cancellation.CancellationSourceKind.CACHE_PUSH_WATCHDOG
        )
        assert result.run_id == 44

    def test_repo_has_no_direct_workflow_cancellation_calls(self) -> None:
        allowed = {MODULE_PATH.resolve(), Path(__file__).resolve()}
        offenders: list[str] = []
        for path in REPOSITORY.rglob("*"):
            if path.resolve() in allowed or not path.is_file():
                continue
            if path.suffix not in {
                ".js",
                ".mjs",
                ".nix",
                ".py",
                ".sh",
                ".yaml",
                ".yml",
            }:
                continue
            source_text = path.read_text(errors="replace")
            has_gh_cancel = "gh run" + " cancel" in source_text
            has_api_cancel = (
                re.search(r"actions/runs/[^\n]*/cancel\b", source_text) is not None
            )
            if has_gh_cancel or has_api_cancel:
                offenders.append(str(path.relative_to(REPOSITORY)))
        assert offenders == []


if __name__ == "__main__":
    unittest.main()
