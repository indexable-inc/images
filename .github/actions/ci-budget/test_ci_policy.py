from __future__ import annotations

import json
import tempfile
import unittest
from datetime import UTC, datetime
from pathlib import Path

from ci_policy import (
    POLICY,
    check_consumer,
    cli_decision,
    decide,
    load_policy,
    standard_deadline,
    standard_minutes,
    sync_owner_revision,
    worker_timeout_minutes,
)


def valid_policy() -> dict[str, object]:
    return {
        "big_change_label": "ci/big-change",
        "extended_setup_allowance_seconds": 120,
        "extended_validation_seconds": 10_800,
        "repositories": {
            "indexable-inc/index": {
                "costly_paths": ["flake.lock"],
                "managed_workflows": [".github/workflows/check.yml"],
                "merge_queue_branch": "main",
            }
        },
        "standard_seconds": 300,
        "termination_grace_seconds": 60,
    }


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

    def test_reusable_workflows_load_the_action_from_their_exact_version(self) -> None:
        workflows = Path(__file__).resolve().parents[2] / "workflows"
        for name in ("ci-budget.yml", "ci-budget-read-only.yml"):
            with self.subTest(workflow=name):
                source = (workflows / name).read_text()
                assert "repository: ${{ job.workflow_repository }}" in source
                assert "ref: ${{ job.workflow_sha }}" in source
                assert "uses: ./.ci-budget-owner/.github/actions/ci-budget" in source
                assert (
                    "uses: indexable-inc/index/.github/actions/ci-budget@main"
                    not in source
                )

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
        assert decision.big_change_label == "ci/big-change"
        assert decision.classification.reason["sources"] == ["costly_path"]

    def test_owner_repository_does_not_reimplement_total_deadline_enforcement(
        self,
    ) -> None:
        repository = Path(__file__).resolve().parents[3]

        check_consumer(repository, "indexable-inc/index")

    def test_consumer_contract_requires_pinned_independent_controller(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            revision = "a" * 40
            (root / "flake.lock").write_text(
                json.dumps(
                    {
                        "root": "root",
                        "nodes": {
                            "root": {"inputs": {"index": "index"}},
                            "index": {
                                "locked": {
                                    "owner": "indexable-inc",
                                    "repo": "index",
                                    "rev": revision,
                                }
                            },
                        },
                    }
                )
            )
            workflows = root / ".github" / "workflows"
            workflows.mkdir(parents=True)
            controller = workflows / "ci-deadline-controller.yml"
            controller.write_text(
                "workflow_run:\n"
                "  types: [requested, in_progress]\n"
                "runs-on: ubuntu-latest\n"
                "actions: write\n"
                "uses: indexable-inc/index/.github/actions/ci-budget@main\n"
                "mode: cancel\n"
            )

            with self.assertRaisesRegex(  # noqa: PT027  # plain unittest suite
                RuntimeError, "root index input revision"
            ):
                check_consumer(root, "indexable-inc/ix")

            changed = sync_owner_revision(root, "indexable-inc/ix")
            assert changed == (Path(".github/workflows/ci-deadline-controller.yml"),)
            assert f"ci-budget@{revision}" in controller.read_text()
            check_consumer(root, "indexable-inc/ix")

    def test_consumer_contract_rejects_deadline_arithmetic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / "scripts" / "ci" / "deadline.py"
            script.parent.mkdir(parents=True)
            script.write_text("deadline = created_at + budget\n")

            with self.assertRaisesRegex(  # noqa: PT027  # plain unittest suite
                RuntimeError, "deadline arithmetic"
            ):
                check_consumer(root, "indexable-inc/index")

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
        assert decision.standard_seconds == 300

    def test_policy_rejects_unknown_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps({"standard_seconds": 300}))

            assert "policy root keys" in str(policy_error(path))

    def test_policy_rejects_boolean_integer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            policy = valid_policy()
            policy["standard_seconds"] = True
            path.write_text(json.dumps(policy))

            assert "positive integer" in str(policy_error(path))


if __name__ == "__main__":
    unittest.main()
