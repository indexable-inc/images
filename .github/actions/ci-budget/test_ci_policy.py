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
        "notes": {
            "extended_validation_seconds": "why",
            "queue_start_seconds": "why",
            "routine_validation_seconds": "why",
            "setup_allowance_seconds": "why",
            "termination_grace_seconds": "why",
        },
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
        assert queue_start_minutes() == 120
        # The extended tier and every clock outside validation stay shared; only
        # the routine tier is measured per repository, so these are the numbers
        # that must not drift apart between repositories.
        for repository in ("indexable-inc/ix", "indexable-inc/index"):
            with self.subTest(repository=repository):
                assert validation_seconds(repository, big_change=True) == 10_800
                assert worker_timeout_minutes(repository, big_change=True) == 183

    def test_routine_budget_is_measured_per_repository(self) -> None:
        # ix's gate has passed in as much as 5012s where index's slowest pass was
        # 1862s (see catalog/policy.json notes). One shared routine number cannot
        # be right for both, and the previous shared 300s was killing 54.5% of
        # the ix gates that went on to pass.
        assert validation_seconds("indexable-inc/ix", big_change=False) == 5_400
        assert worker_timeout_minutes("indexable-inc/ix", big_change=False) == 93
        assert validation_seconds("indexable-inc/index", big_change=False) == 2_400
        assert worker_timeout_minutes("indexable-inc/index", big_change=False) == 43

    def test_routine_budget_falls_back_to_the_catalog_default(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps(valid_policy()))

            policy = load_policy(path)

            assert policy.routine_seconds("indexable-inc/index") == 300

    def test_a_threshold_without_its_evidence_fails_the_catalog(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            policy = valid_policy()
            repositories = policy["repositories"]
            assert isinstance(repositories, dict)
            repositories["indexable-inc/index"]["routine_validation_seconds"] = 900
            path.write_text(json.dumps(policy))

            assert "notes keys must be" in str(policy_error(path))

    def test_an_orphaned_note_fails_the_catalog(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            policy = valid_policy()
            notes = policy["notes"]
            assert isinstance(notes, dict)
            notes["repositories.indexable-inc/nope.routine_validation_seconds"] = "why"
            path.write_text(json.dumps(policy))

            assert "notes keys must be" in str(policy_error(path))

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
        # The default moved from a hosted runner to a dispatcher claim in
        # c58cb13; what this guards is that `runner-label` still overrides it,
        # not which runner the default names.
        assert (
            'runs-on: ["${{ inputs.runner-label || '
            "format('ix-ci-run-{0}-{1}-update-flake-lock', "
            'github.run_id, github.run_attempt) }}"]' in workflow
        )
        # `nix-preinstalled` is accepted and ignored now that every runner this
        # job lands on provides Nix; it survives only so indexable-inc/ix's
        # existing caller keeps validating. Guard that it is still accepted, not
        # that it still gates a step.
        assert "Accepted and ignored" in workflow

    def test_owner_policy_classifies_repository_data(self) -> None:
        costly_paths = (
            ".cargo/config.toml",
            ".github/workflows/ci.yml",
            "Cargo.lock",
            "Cargo.toml",
            "flake.lock",
            "flake.nix",
            "crates/vm/guest/console/terminal/zig/deps.nix",
            "nix/checks/default.nix",
            "lib/workspace-cargo-unit.nix",
            "nix/flake/outputs/default.nix",
            "nix/flake/outputs/workspace.nix",
            "nix/packages/workspace-binaries.json",
            "nix/packages/workspace-bins.nix",
            "nix/packages/workspace-bins/example.nix",
            "nix/packages/workspace-rust-ci.nix",
            "nix/shells/toolchain.nix",
            "rust-toolchain.toml",
        )
        for path in costly_paths:
            with self.subTest(path=path):
                decision = decide(
                    [path],
                    [],
                    "indexable-inc/ix",
                    ".github/workflows/ci.yml",
                    force_big_change=False,
                )

                assert decision.managed_workflow
                assert decision.classification.big_change
                assert decision.classification.reason["sources"] == ["costly_path"]

        routine_paths = (
            ".github/actions/ci-setup/action.yml",
            "docs/operators/ci.md",
            "nix/checks/workflow-ci-one-claim.jq",
            "scripts/ci/run-ci-phases.sh",
        )
        for path in routine_paths:
            with self.subTest(routine_path=path):
                routine = decide(
                    [path],
                    [],
                    "indexable-inc/ix",
                    ".github/workflows/ci.yml",
                    force_big_change=False,
                )
                assert routine.managed_workflow
                assert not routine.classification.big_change

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
        assert decision.queue_start_seconds == 7200
        assert decision.setup_allowance_seconds == 120
        assert decision.validation_seconds == 10_800
        assert decision.termination_grace_seconds == 10
        assert decision.worker_timeout_minutes == 183

    def test_cli_contract_carries_the_repository_routine_budget(self) -> None:
        decision = cli_decision(
            {
                "changed_paths": ["nix/modules/services/host/ci-dispatcher/module.nix"],
                "force_big_change": False,
                "labels": [],
                "repository": "indexable-inc/ix",
                "workflow_path": ".github/workflows/ci.yml",
            },
            POLICY,
        )

        assert not decision.classification.big_change
        assert decision.validation_seconds == 5_400
        assert decision.worker_timeout_minutes == 93

    def test_policy_rejects_unknown_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            path.write_text(json.dumps({"queue_start_seconds": 300}))

            assert "policy root keys" in str(policy_error(path))

    def test_policy_rejects_boolean_integer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "policy.json"
            policy = valid_policy()
            policy["setup_allowance_seconds"] = True
            path.write_text(json.dumps(policy))

            assert "positive integer" in str(policy_error(path))


if __name__ == "__main__":
    unittest.main()
