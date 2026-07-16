from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from ci_policy import (
    POLICY,
    cli_decision,
    decide,
    load_policy,
    queue_start_minutes,
    validation_seconds,
    worker_timeout_minutes,
)


def valid_policy() -> dict[str, object]:
    return {
        "big_change_label": "ci/big-change",
        "extended_validation_seconds": 10_800,
        "queue_start_seconds": 300,
        "repositories": {
            "indexable-inc/index": {
                "costly_paths": ["flake.lock"],
                "managed_workflows": [".github/workflows/check.yml"],
            }
        },
        "routine_validation_seconds": 300,
        "setup_allowance_seconds": 120,
        "termination_grace_seconds": 10,
    }


def policy_error(path: Path) -> RuntimeError:
    try:
        load_policy(path)
    except RuntimeError as error:
        return error
    raise AssertionError("policy did not fail")


class PolicyTests(unittest.TestCase):
    def test_shared_phase_clocks_and_worker_envelopes(self) -> None:
        assert queue_start_minutes() == 5
        assert validation_seconds(big_change=False) == 300
        assert validation_seconds(big_change=True) == 10_800
        assert worker_timeout_minutes(big_change=False) == 8
        assert worker_timeout_minutes(big_change=True) == 183

    def test_reusable_workflows_load_the_action_from_their_exact_version(self) -> None:
        workflows = Path(__file__).resolve().parents[2] / "workflows"
        for name in ("ci-budget.yml", "ci-budget-read-only.yml"):
            with self.subTest(workflow=name):
                source = (workflows / name).read_text()
                assert "repository: ${{ job.workflow_repository }}" in source
                assert "ref: ${{ job.workflow_sha }}" in source
                assert "uses: ./.ci-budget-owner/.github/actions/ci-budget" in source
                assert "runner-label:" in source
                assert "default: ubuntu-latest" in source
                assert "runs-on: ${{ inputs.runner-label }}" in source
                assert (
                    "uses: indexable-inc/index/.github/actions/ci-budget@main"
                    not in source
                )

    def test_update_workflow_accepts_a_nix_owned_runner(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[2] / "workflows" / "update-flake-lock.yml"
        ).read_text()
        assert "runner-label:" in workflow
        assert "nix-preinstalled:" in workflow
        assert "runs-on: ${{ inputs.runner-label || 'ubuntu-latest' }}" in workflow
        assert "if: ${{ !inputs.nix-preinstalled }}" in workflow

    def test_owner_policy_classifies_repository_data(self) -> None:
        decision = decide(
            ["nix/packages/workspace-bins.nix"],
            [],
            "indexable-inc/ix",
            ".github/workflows/ci.yml",
            force_big_change=False,
        )

        assert decision.managed_workflow
        assert decision.classification.big_change
        assert decision.classification.reason["sources"] == ["costly_path"]

    def test_cli_contract_rejects_unmanaged_workflow_without_reclassifying(
        self,
    ) -> None:
        decision = cli_decision(
            {
                "changed_paths": ["flake.lock"],
                "force_big_change": False,
                "labels": [],
                "repository": "indexable-inc/index",
                "workflow_path": ".github/workflows/pages.yml",
            },
            POLICY,
        )

        assert not decision.managed_workflow
        assert decision.classification.big_change
        assert decision.queue_start_seconds == 300
        assert decision.setup_allowance_seconds == 120
        assert decision.validation_seconds == 10_800
        assert decision.termination_grace_seconds == 10
        assert decision.worker_timeout_minutes == 183

    def test_policy_rejects_unknown_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps({"queue_start_seconds": 300}))

            assert "policy root keys" in str(policy_error(path))

    def test_policy_rejects_boolean_integer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            policy = valid_policy()
            policy["queue_start_seconds"] = True
            path.write_text(json.dumps(policy))

            assert "positive integer" in str(policy_error(path))


if __name__ == "__main__":
    unittest.main()
