#!/usr/bin/env python3
"""Single CI budget policy and GitHub context resolver entry point."""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping, Sequence

import ci_budget
import ci_policy


def positive_int(value: object, name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"resolver input {name} must be a positive integer")
    return value


def non_empty_string(value: object, name: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"resolver input {name} must be a non-empty string")
    return value


def object_input(value: object) -> Mapping[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError("resolver input must be an object")
    expected = {"repository", "run_attempt", "run_id", "token"}
    if set(value) != expected:
        raise ValueError(
            f"resolver input keys must be {sorted(expected)}, got {sorted(value)}"
        )
    return value


def resolved_decision(payload: object) -> dict[str, object]:
    value = object_input(payload)
    repository = non_empty_string(value["repository"], "repository")
    run_id = positive_int(value["run_id"], "run_id")
    run_attempt = positive_int(value["run_attempt"], "run_attempt")
    token = non_empty_string(value["token"], "token")
    client = ci_budget.GitHubClient(repository, token)
    resolved = ci_budget.resolve_budget(
        client,
        ci_budget.ResolutionRequest(
            repository=repository,
            run_id=run_id,
            run_attempt=run_attempt,
        ),
    )
    attempt_run_id = positive_int(resolved.attempt.get("id"), "attempt.id")
    attempt_number = positive_int(
        resolved.attempt.get("run_attempt"), "attempt.run_attempt"
    )
    if attempt_run_id != run_id or attempt_number != run_attempt:
        raise RuntimeError(
            "GitHub workflow attempt identity disagrees with resolver input"
        )
    run = client.workflow_run(run_id)
    created_at = ci_policy.parse_timestamp(run.get("created_at"), "created_at")
    head_sha = non_empty_string(resolved.attempt.get("head_sha"), "attempt.head_sha")
    ci_budget.validate_sha(head_sha, "workflow head SHA")
    workflow_path = ci_budget.workflow_path(resolved.attempt)
    decision = ci_policy.decision_from_classification(
        resolved.classification,
        repository,
        workflow_path,
    )
    return {
        **decision.to_json(),
        "created_at": created_at.isoformat(),
        "head_sha": head_sha,
        "pull_request_number": resolved.pull_request_number,
        "repository": repository,
        "run_attempt": run_attempt,
        "run_id": run_id,
        "workflow_path": workflow_path,
    }


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments == ["--action"]:
        return ci_budget.main()
    if arguments == ["--runtime-policy"]:
        json.dump(
            ci_policy.runtime_policy_json(),
            sys.stdout,
            separators=(",", ":"),
            sort_keys=True,
        )
        sys.stdout.write("\n")
        return 0
    if arguments == ["--resolve"]:
        json.dump(
            resolved_decision(json.load(sys.stdin)),
            sys.stdout,
            separators=(",", ":"),
            sort_keys=True,
        )
        sys.stdout.write("\n")
        return 0
    return ci_policy.main(arguments)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ci-budget-policy: {error}", file=sys.stderr)
        sys.exit(1)
