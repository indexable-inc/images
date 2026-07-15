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


def child_list(parent: JsonObject, key: str) -> list[Any]:
    child = parent.get(key)
    if not isinstance(child, list):
        raise AssertionError(f"{key!r} is not a list")
    return child


def events(workflow: JsonObject) -> JsonObject:
    value = workflow.get("on", workflow.get("true"))
    if not isinstance(value, dict):
        raise AssertionError("workflow has no event object")
    return value


def expression(parent: JsonObject, key: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str):
        raise AssertionError(f"{key!r} is not an expression")
    return " ".join(value.split())


def all_uses(value: object) -> list[str]:
    if isinstance(value, dict):
        own = value.get("uses")
        result = [own] if isinstance(own, str) else []
        for child in value.values():
            result.extend(all_uses(child))
        return result
    if isinstance(value, list):
        result: list[str] = []
        for child in value:
            result.extend(all_uses(child))
        return result
    return []


class RequiredWorkflowTests(unittest.TestCase):
    def test_required_context_is_an_unconditional_terminal_gate(self) -> None:
        cases = (
            ("check.yml", "flake-build", "flake-check"),
            ("closure-gate.yml", "closure-build", "closure-gate"),
        )
        for workflow_name, target_name, gate_name in cases:
            with self.subTest(workflow=workflow_name):
                workflow = load_workflow(workflow_name)
                jobs = child_object(workflow, "jobs")
                budget = child_object(jobs, "ci-budget")
                budget_permissions = child_object(budget, "permissions")
                target = child_object(jobs, target_name)
                gate = child_object(jobs, gate_name)
                gate_permissions = child_object(gate, "permissions")
                step = child_list(gate, "steps")[0]
                if not isinstance(step, dict):
                    raise AssertionError("gate step is not an object")
                inputs = child_object(step, "with")

                assert (
                    budget["uses"]
                    == "indexable-inc/index/.github/workflows/ci-budget-read-only.yml@main"
                )
                assert budget_permissions == {
                    "actions": "read",
                    "contents": "read",
                    "pull-requests": "read",
                }
                assert target["name"] == target_name
                assert target["needs"] == "ci-budget"
                assert gate["name"] == gate_name
                assert gate["needs"] == ["ci-budget", target_name]
                assert expression(gate, "if") == "${{ always() }}"
                assert gate["runs-on"] == "ubuntu-latest"
                assert gate["timeout-minutes"] == standard_minutes()
                assert gate_permissions == {"actions": "read", "contents": "read"}
                assert (
                    step["uses"] == "indexable-inc/index/.github/actions/ci-budget@main"
                )
                assert inputs["mode"] == "gate"
                assert inputs["target-job-name"] == target_name
                assert expression(inputs, "run-id") == "${{ github.run_id }}"
                assert expression(inputs, "run-attempt") == "${{ github.run_attempt }}"
                assert "needs.ci-budget.outputs.big_change" in expression(
                    inputs, "big-change"
                )
                assert isinstance(target["timeout-minutes"], int)
                assert target["timeout-minutes"] > standard_minutes()

    def test_required_workflows_are_read_only_on_pull_requests(self) -> None:
        for workflow_name in ("check.yml", "closure-gate.yml"):
            with self.subTest(workflow=workflow_name):
                workflow = load_workflow(workflow_name)
                assert "pull_request" in events(workflow)
                assert "pull_request_target" not in events(workflow)
                assert child_object(workflow, "permissions") == {"contents": "read"}
                for job in child_object(workflow, "jobs").values():
                    if not isinstance(job, dict) or "permissions" not in job:
                        continue
                    permissions = child_object(job, "permissions")
                    assert "write" not in permissions.values()

    def test_non_pull_request_classification_keeps_labels(self) -> None:
        check_inputs = child_object(
            child_object(child_object(load_workflow("check.yml"), "jobs"), "ci-budget"),
            "with",
        )
        assert "event.before" in expression(check_inputs, "base-sha")
        assert "github.sha" in expression(check_inputs, "head-sha")
        assert "merge_group.head_sha" in expression(check_inputs, "head-sha")
        assert "refs/tags/" in expression(check_inputs, "force-big-change")

        closure_inputs = child_object(
            child_object(
                child_object(load_workflow("closure-gate.yml"), "jobs"), "ci-budget"
            ),
            "with",
        )
        assert "base-sha" not in closure_inputs
        assert "merge_group.head_sha" in expression(closure_inputs, "head-sha")
        assert "workflow_dispatch" in expression(closure_inputs, "force-big-change")


class TrustedWorkflowTests(unittest.TestCase):
    def test_controller_covers_initial_runs_and_reruns(self) -> None:
        workflow = load_workflow("ci-deadline-controller.yml")
        trigger = child_object(events(workflow), "workflow_run")
        assert set(child_list(trigger, "workflows")) == {"Check", "Closure gate"}
        assert set(child_list(trigger, "types")) == {"requested", "in_progress"}

        concurrency = child_object(workflow, "concurrency")
        assert concurrency["cancel-in-progress"] is True
        assert "workflow_run.id" in expression(concurrency, "group")
        assert "workflow_run.run_attempt" in expression(concurrency, "group")

        deadline = child_object(child_object(workflow, "jobs"), "deadline")
        assert (
            deadline["uses"]
            == "indexable-inc/index/.github/workflows/ci-deadline.yml@main"
        )
        assert child_object(deadline, "permissions") == {
            "actions": "write",
            "contents": "read",
            "pull-requests": "read",
        }
        inputs = child_object(deadline, "with")
        assert "workflow_run.id" in expression(inputs, "run-id")
        assert "workflow_run.run_attempt" in expression(inputs, "run-attempt")
        assert not any(use.startswith("actions/checkout") for use in all_uses(workflow))

    def test_publisher_uses_trusted_base_code_without_checkout(self) -> None:
        workflow = load_workflow("ci-budget-publish.yml")
        assert "pull_request_target" in events(workflow)
        publish = child_object(child_object(workflow, "jobs"), "publish")
        assert (
            publish["uses"]
            == "indexable-inc/index/.github/workflows/ci-budget.yml@main"
        )
        assert child_object(publish, "permissions") == {
            "actions": "read",
            "contents": "read",
            "issues": "write",
            "pull-requests": "write",
        }
        assert not any(use.startswith("actions/checkout") for use in all_uses(workflow))

    def test_budget_check_watches_owner_and_consumers(self) -> None:
        paths = child_list(
            child_object(events(load_workflow("ci-budget-check.yml")), "pull_request"),
            "paths",
        )
        assert {
            ".github/actions/ci-budget/**",
            ".github/workflows/check.yml",
            ".github/workflows/closure-gate.yml",
            ".github/workflows/ci-budget-publish.yml",
            ".github/workflows/ci-deadline-controller.yml",
            ".github/workflows/ci-deadline.yml",
        } <= set(paths)


if __name__ == "__main__":
    unittest.main()
