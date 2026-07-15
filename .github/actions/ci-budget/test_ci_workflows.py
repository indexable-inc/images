from __future__ import annotations

import json
import subprocess
import unittest
from pathlib import Path
from typing import Any

from ci_policy import standard_minutes

ACTION_DIR = Path(__file__).resolve().parent
REPOSITORY = ACTION_DIR.parents[2]
WORKFLOWS = REPOSITORY / ".github" / "workflows"
YAML_TO_JSON = ACTION_DIR / "workflow_yaml_to_json.rb"

JsonObject = dict[str, Any]


def load_workflow(name: str) -> JsonObject:
    path = WORKFLOWS / name
    result = subprocess.run(
        ["ruby", str(YAML_TO_JSON)],
        input=path.read_text(),
        text=True,
        capture_output=True,
        check=True,
    )
    decoded = json.loads(result.stdout)
    if not isinstance(decoded, dict):
        raise AssertionError(f"{path} did not decode to an object")
    return decoded


def child_object(parent: JsonObject, key: str) -> JsonObject:
    child = parent.get(key)
    if not isinstance(child, dict):
        raise AssertionError(f"{key!r} is not an object")
    return child


def expression(parent: JsonObject, key: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str):
        raise AssertionError(f"{key!r} is not an expression")
    return " ".join(value.split())


class RequiredWorkflowTests(unittest.TestCase):
    def test_required_jobs_run_beside_shared_deadline(self) -> None:
        cases = (
            ("check.yml", "flake-check"),
            ("closure-gate.yml", "closure-gate"),
        )
        for workflow_name, target_name in cases:
            with self.subTest(workflow=workflow_name):
                jobs = child_object(load_workflow(workflow_name), "jobs")
                deadline = child_object(jobs, "ci-deadline")
                target = child_object(jobs, target_name)
                inputs = child_object(deadline, "with")

                assert deadline["needs"] == target["needs"] == "ci-budget"
                assert (
                    deadline["uses"]
                    == "indexable-inc/index/.github/workflows/ci-deadline.yml@main"
                )
                assert (
                    expression(deadline, "if")
                    == "needs.ci-budget.outputs.big_change != 'true'"
                )
                assert child_object(deadline, "permissions")["actions"] == "write"
                assert json.loads(expression(inputs, "target-job-names")) == [
                    target_name
                ]
                assert expression(inputs, "run-id") == "${{ github.run_id }}"
                assert expression(inputs, "run-attempt") == "${{ github.run_attempt }}"
                assert isinstance(target["timeout-minutes"], int)
                assert target["timeout-minutes"] > standard_minutes()

    def test_events_classify_ranges_and_preserve_extended_runs(self) -> None:
        check_jobs = child_object(load_workflow("check.yml"), "jobs")
        check_budget = child_object(check_jobs, "ci-budget")
        check_inputs = child_object(check_budget, "with")
        assert (
            check_budget["uses"]
            == "indexable-inc/index/.github/workflows/ci-budget.yml@main"
        )
        assert "pull_request.number" in expression(check_inputs, "pull-request-number")
        assert "github.event.before" in expression(check_inputs, "base-sha")
        assert "merge_group.base_sha" in expression(check_inputs, "base-sha")
        assert "github.sha" in expression(check_inputs, "head-sha")
        assert "merge_group.head_sha" in expression(check_inputs, "head-sha")
        assert "refs/tags/" in expression(check_inputs, "force-big-change")

        closure_jobs = child_object(load_workflow("closure-gate.yml"), "jobs")
        closure_budget = child_object(closure_jobs, "ci-budget")
        closure_inputs = child_object(closure_budget, "with")
        assert (
            closure_budget["uses"]
            == "indexable-inc/index/.github/workflows/ci-budget-read-only.yml@main"
        )
        assert "merge_group.base_sha" in expression(closure_inputs, "base-sha")
        assert "merge_group.head_sha" in expression(closure_inputs, "head-sha")
        assert "workflow_dispatch" in expression(closure_inputs, "force-big-change")

    def test_budget_check_watches_owner_and_consumers(self) -> None:
        workflow = load_workflow("ci-budget-check.yml")
        events = workflow.get("on", workflow.get("true"))
        if not isinstance(events, dict):
            raise AssertionError("ci-budget-check.yml has no event object")
        paths = child_object(events, "pull_request").get("paths")
        if not isinstance(paths, list):
            raise AssertionError("ci-budget-check.yml has no pull request paths")

        assert {
            ".github/actions/ci-budget/**",
            ".github/workflows/check.yml",
            ".github/workflows/closure-gate.yml",
            ".github/workflows/ci-deadline.yml",
        } <= set(paths)


if __name__ == "__main__":
    unittest.main()
