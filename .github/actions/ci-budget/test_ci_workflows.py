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


def load_yaml(path: Path) -> JsonObject:
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


def load_workflow(name: str) -> JsonObject:
    return load_yaml(WORKFLOWS / name)


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
                target_permissions = child_object(target, "permissions")
                target_steps = child_list(target, "steps")
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
                assert target_permissions == {
                    "actions": "read",
                    "contents": "read",
                }
                expiry = target_steps[0]
                if not isinstance(expiry, dict):
                    raise AssertionError("expiry step is not an object")
                expiry_inputs = child_object(expiry, "with")
                assert expiry["name"] == "Reject work assigned after the total deadline"
                assert (
                    expiry["uses"]
                    == "indexable-inc/index/.github/actions/ci-budget/worker@main"
                )
                assert expression(expiry_inputs, "run-id") == "${{ github.run_id }}"
                assert expression(expiry_inputs, "run-attempt") == (
                    "${{ github.run_attempt }}"
                )
                assert "needs.ci-budget.outputs.big_change" in expression(
                    expiry_inputs, "big-change"
                )
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
                assert expression(target, "timeout-minutes") == (
                    "${{ fromJSON(needs.ci-budget.outputs.worker_timeout_minutes) }}"
                )

    def test_shared_workflows_publish_the_worker_envelope(self) -> None:
        for workflow_name, job_name in (
            ("ci-budget-read-only.yml", "classify"),
            ("ci-budget.yml", "publish"),
        ):
            with self.subTest(workflow=workflow_name):
                workflow = load_workflow(workflow_name)
                workflow_call = child_object(events(workflow), "workflow_call")
                call_outputs = child_object(workflow_call, "outputs")
                job = child_object(child_object(workflow, "jobs"), job_name)
                job_outputs = child_object(job, "outputs")

                assert "worker_timeout_minutes" in call_outputs
                assert expression(job_outputs, "worker_timeout_minutes") == (
                    "${{ steps.budget.outputs.worker_timeout_minutes }}"
                )

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

    def test_retry_cannot_cancel_a_newer_source_run(self) -> None:
        for workflow_name in ("check.yml", "closure-gate.yml"):
            with self.subTest(workflow=workflow_name):
                workflow = load_workflow(workflow_name)
                concurrency = child_object(workflow, "concurrency")
                group = expression(concurrency, "group")
                cancellation = expression(concurrency, "cancel-in-progress")

                assert "github.run_attempt == 1" in group
                assert "'latest'" in group
                assert "github.run_id" in group
                assert cancellation == (
                    "${{ github.event_name == 'pull_request' "
                    "&& github.run_attempt == 1 }}"
                )

    def test_clone_diff_step_stays_on_the_build_job(self) -> None:
        jobs = child_object(load_workflow("check.yml"), "jobs")
        build_steps = child_list(child_object(jobs, "flake-build"), "steps")
        assert any(
            isinstance(step, dict)
            and step.get("name") == "Reject duplication on changed lines"
            for step in build_steps
        )

    def test_nix_work_runs_inside_the_shared_worker_boundary(self) -> None:
        cases = {
            "check.yml": {
                ".github/scripts/run-clone-diff.sh": [],
                ".github/scripts/run-check-logged.sh": [],
            },
            "closure-gate.yml": {
                ".github/scripts/run-check-logged.sh": ["closure"],
            },
        }
        for workflow_name, expected in cases.items():
            with self.subTest(workflow=workflow_name):
                workflow = load_workflow(workflow_name)
                scripts: dict[str, list[str]] = {}
                for job in child_object(workflow, "jobs").values():
                    if not isinstance(job, dict) or not isinstance(
                        job.get("steps"), list
                    ):
                        continue
                    for step in job["steps"]:
                        if not isinstance(step, dict):
                            continue
                        if step.get("uses") != (
                            "indexable-inc/index/.github/actions/ci-budget/worker@main"
                        ):
                            continue
                        inputs = child_object(step, "with")
                        if inputs.get("mode") != "run":
                            continue
                        script = inputs.get("script")
                        if not isinstance(script, str):
                            raise AssertionError("worker run step has no script")
                        arguments = inputs.get("arguments", "[]")
                        if not isinstance(arguments, str):
                            raise AssertionError("worker arguments are not JSON text")
                        decoded = json.loads(arguments)
                        if not isinstance(decoded, list) or not all(
                            isinstance(item, str) for item in decoded
                        ):
                            raise AssertionError("worker arguments are not strings")
                        scripts[script] = decoded
                assert scripts == expected

        assert not (REPOSITORY / ".github/actions/check-logged/action.yml").exists()

    def test_non_pull_request_classification_keeps_labels(self) -> None:
        check_inputs = child_object(
            child_object(child_object(load_workflow("check.yml"), "jobs"), "ci-budget"),
            "with",
        )
        assert "event.before" in expression(check_inputs, "base-sha")
        assert "merge_group.base_sha" in expression(check_inputs, "base-sha")
        assert "github.sha" in expression(check_inputs, "head-sha")
        assert "merge_group.head_sha" in expression(check_inputs, "head-sha")
        assert "refs/tags/" in expression(check_inputs, "force-big-change")

        closure_inputs = child_object(
            child_object(
                child_object(load_workflow("closure-gate.yml"), "jobs"), "ci-budget"
            ),
            "with",
        )
        assert "merge_group.base_sha" in expression(closure_inputs, "base-sha")
        assert "merge_group.head_sha" in expression(closure_inputs, "head-sha")
        assert "workflow_dispatch" in expression(closure_inputs, "force-big-change")


class TrustedWorkflowTests(unittest.TestCase):
    def test_source_artifact_snapshots_the_classified_budget(self) -> None:
        action = load_yaml(ACTION_DIR / "action.yml")
        steps = child_list(child_object(action, "runs"), "steps")
        preserve = next(
            step
            for step in steps
            if isinstance(step, dict)
            and step.get("name") == "Preserve classified budget"
        )
        inputs = child_object(preserve, "with")

        assert expression(preserve, "if") == "inputs.mode == 'classify'"
        assert "steps.classify.outputs.snapshot_key" in expression(inputs, "name")
        assert (
            expression(inputs, "path") == "${{ steps.classify.outputs.snapshot_path }}"
        )

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
        assert deadline["runs-on"] == "ubuntu-latest"
        assert deadline["timeout-minutes"] == 10
        assert child_object(deadline, "permissions") == {
            "actions": "write",
            "contents": "read",
        }
        step = child_list(deadline, "steps")[0]
        if not isinstance(step, dict):
            raise AssertionError("deadline step is not an object")
        assert step["uses"] == "indexable-inc/index/.github/actions/ci-budget@main"
        inputs = child_object(step, "with")
        assert inputs["mode"] == "cancel"
        assert "workflow_run.id" in expression(inputs, "run-id")
        assert "workflow_run.run_attempt" in expression(inputs, "run-attempt")
        assert not any(use.startswith("actions/checkout") for use in all_uses(workflow))

    def test_cancel_mode_always_uploads_its_audit_record(self) -> None:
        action = load_yaml(ACTION_DIR / "action.yml")
        steps = child_list(child_object(action, "runs"), "steps")
        enforce = next(
            step
            for step in steps
            if isinstance(step, dict)
            and step.get("name") == "Enforce total CI deadline"
        )
        preserve = next(
            step
            for step in steps
            if isinstance(step, dict)
            and step.get("name") == "Preserve workflow cancellation records"
        )
        environment = child_object(enforce, "env")
        inputs = child_object(preserve, "with")

        assert "workflow-cancellations" in expression(
            environment, "WORKFLOW_CANCELLATION_RECORD_DIRECTORY"
        )
        assert expression(preserve, "if") == "inputs.mode == 'cancel' && always()"
        assert preserve["uses"].startswith("actions/upload-artifact@")
        assert "inputs.run-id" in expression(inputs, "name")
        assert inputs["if-no-files-found"] == "ignore"

    def test_watchdog_routes_cancellation_through_the_typed_owner(self) -> None:
        workflow = load_workflow("cache-push-watchdog.yml")
        watch = child_object(child_object(workflow, "jobs"), "watch")
        environment = child_object(watch, "env")
        steps = child_list(watch, "steps")
        checkout, detect, preserve = steps
        if not all(isinstance(step, dict) for step in steps):
            raise AssertionError("watchdog step is not an object")
        script = detect.get("run")
        if not isinstance(script, str):
            raise AssertionError("watchdog detection step has no script")

        assert checkout["uses"].startswith("actions/checkout@")
        assert environment["WORKFLOW_CANCELLATION_SOURCE"] == "cache_push_watchdog"
        assert "workflow_cancellation.py" in script
        assert "cache_push_zombie" in script
        assert "cache_push_materialization_stall" in script
        assert "gh run" + " cancel" not in script
        assert expression(preserve, "if") == "always()"
        assert preserve["uses"].startswith("actions/upload-artifact@")

    def test_publisher_uses_trusted_base_code_without_checkout(self) -> None:
        workflow = load_workflow("ci-budget-publish.yml")
        assert "pull_request_target" in events(workflow)
        concurrency = child_object(workflow, "concurrency")
        assert "pull_request.number" in expression(concurrency, "group")
        assert concurrency["cancel-in-progress"] is False
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
            ".github/scripts/run-check-logged.sh",
            ".github/scripts/run-clone-diff.sh",
            ".github/workflows/cache-push-watchdog.yml",
            ".github/workflows/check.yml",
            ".github/workflows/closure-gate.yml",
            ".github/workflows/ci-budget-publish.yml",
            ".github/workflows/ci-deadline-controller.yml",
        } <= set(paths)


if __name__ == "__main__":
    unittest.main()
