from __future__ import annotations

import importlib.util
import io
import json
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

MODULE_PATH = Path(__file__).with_name("resolver.py")
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("ci_budget_resolver", MODULE_PATH)
assert SPEC is not None
assert SPEC.loader is not None
resolver = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = resolver
SPEC.loader.exec_module(resolver)


class ResolverContractTests(unittest.TestCase):
    def test_runtime_policy_exports_dispatcher_bootstrap_contract(self) -> None:
        output = io.StringIO()
        with redirect_stdout(output):
            status = resolver.main(["--runtime-policy"])

        assert status == 0
        assert json.loads(output.getvalue()) == {
            "schema_version": 1,
            "big_change_label": "ci/big-change",
            "standard_seconds": 300,
            "termination_grace_seconds": 60,
            "repositories": {
                "indexable-inc/index": {
                    "managed_workflows": [
                        ".github/workflows/check.yml",
                        ".github/workflows/closure-gate.yml",
                    ]
                },
                "indexable-inc/ix": {"managed_workflows": [".github/workflows/ci.yml"]},
            },
        }

    def test_typed_output_freezes_identity_policy_and_reason(self) -> None:
        resolved = resolver.ci_budget.ResolvedBudget(
            attempt={
                "id": 12,
                "run_attempt": 1,
                "created_at": "2026-07-15T10:00:00Z",
                "head_sha": "a" * 40,
                "path": ".github/workflows/ci.yml",
            },
            classification=resolver.ci_policy.Classification(
                big_change=True,
                reason={
                    "sources": ["costly_path"],
                    "matches": [{"path": "flake.lock", "pattern": "flake.lock"}],
                },
            ),
            labels=(),
            pull_request_number=42,
        )
        with (
            mock.patch.object(
                resolver.ci_budget, "resolve_budget", return_value=resolved
            ),
            mock.patch.object(
                resolver.ci_budget.GitHubClient,
                "workflow_run",
                return_value={"created_at": "2026-07-15T10:00:00Z"},
            ),
        ):
            decision = resolver.resolved_decision(
                {
                    "repository": "indexable-inc/ix",
                    "run_attempt": 1,
                    "run_id": 12,
                    "token": "secret",
                }
            )

        assert decision == {
            "big_change": True,
            "big_change_label": "ci/big-change",
            "budget_seconds": 10_920,
            "created_at": "2026-07-15T10:00:00+00:00",
            "head_sha": "a" * 40,
            "managed_workflow": True,
            "pull_request_number": 42,
            "reason": {
                "sources": ["costly_path"],
                "matches": [{"path": "flake.lock", "pattern": "flake.lock"}],
            },
            "repository": "indexable-inc/ix",
            "run_attempt": 1,
            "run_id": 12,
            "standard_seconds": 300,
            "termination_grace_seconds": 60,
            "workflow_path": ".github/workflows/ci.yml",
        }

    def test_retry_uses_original_run_creation_not_attempt_creation(self) -> None:
        resolved = resolver.ci_budget.ResolvedBudget(
            attempt={
                "id": 12,
                "run_attempt": 2,
                "created_at": "2026-07-15T13:24:32Z",
                "head_sha": "a" * 40,
                "path": ".github/workflows/ci.yml",
            },
            classification=resolver.ci_policy.Classification(
                big_change=False,
                reason={"sources": [], "matches": []},
            ),
            labels=(),
            pull_request_number=42,
        )
        with (
            mock.patch.object(
                resolver.ci_budget, "resolve_budget", return_value=resolved
            ),
            mock.patch.object(
                resolver.ci_budget.GitHubClient,
                "workflow_run",
                return_value={"created_at": "2026-07-15T12:08:20Z"},
            ),
        ):
            decision = resolver.resolved_decision(
                {
                    "repository": "indexable-inc/ix",
                    "run_attempt": 2,
                    "run_id": 12,
                    "token": "secret",
                }
            )

        assert decision["created_at"] == "2026-07-15T12:08:20+00:00"

    def test_typed_input_rejects_extra_fields(self) -> None:
        with self.assertRaisesRegex(  # noqa: PT027  # plain unittest suite
            ValueError, "resolver input keys"
        ):
            resolver.resolved_decision(
                {
                    "repository": "indexable-inc/ix",
                    "run_attempt": 1,
                    "run_id": 12,
                    "token": "secret",
                    "trusted": False,
                }
            )


if __name__ == "__main__":
    unittest.main()
