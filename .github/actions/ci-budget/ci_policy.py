from __future__ import annotations

import argparse
import fnmatch
import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timedelta
from math import ceil
from pathlib import Path

POLICY_PATH = Path(__file__).with_name("policy.json")


@dataclass(frozen=True)
class RepositoryPolicy:
    costly_paths: tuple[str, ...]
    managed_workflows: tuple[str, ...]


@dataclass(frozen=True)
class Policy:
    big_change_label: str
    standard_seconds: int
    extended_validation_seconds: int
    extended_setup_allowance_seconds: int
    termination_grace_seconds: int
    repositories: Mapping[str, RepositoryPolicy]

    @property
    def extended_worker_minutes(self) -> int:
        seconds = (
            self.extended_validation_seconds
            + self.extended_setup_allowance_seconds
            + self.termination_grace_seconds
        )
        return ceil(seconds / 60)


@dataclass(frozen=True)
class Classification:
    big_change: bool
    reason: dict[str, object]


@dataclass(frozen=True)
class PolicyDecision:
    classification: Classification
    budget_seconds: int
    managed_workflow: bool
    standard_seconds: int
    termination_grace_seconds: int

    def to_json(self) -> dict[str, object]:
        return {
            "big_change": self.classification.big_change,
            "budget_seconds": self.budget_seconds,
            "managed_workflow": self.managed_workflow,
            "reason": self.classification.reason,
            "standard_seconds": self.standard_seconds,
            "termination_grace_seconds": self.termination_grace_seconds,
        }


def positive_int(value: object, name: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise RuntimeError(f"CI budget policy {name} must be a positive integer")
    return value


def non_empty_string(value: object, name: str) -> str:
    if not isinstance(value, str) or not value:
        raise RuntimeError(f"CI budget policy {name} must be a non-empty string")
    return value


def string_tuple(value: object, name: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise RuntimeError(f"CI budget policy {name} must be a non-empty array")
    if not all(isinstance(item, str) and item for item in value):
        raise RuntimeError(f"CI budget policy {name} must contain non-empty strings")
    return tuple(value)


def object_mapping(value: object, name: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise RuntimeError(f"CI budget policy {name} must be an object")
    return value


def exact_keys(value: Mapping[str, object], expected: set[str], name: str) -> None:
    if set(value) != expected:
        raise RuntimeError(
            f"CI budget policy {name} keys must be {sorted(expected)}, "
            f"got {sorted(value)}"
        )


def load_policy(path: Path = POLICY_PATH) -> Policy:
    parsed: object = json.loads(path.read_text())
    root = object_mapping(parsed, "root")
    expected = {
        "big_change_label",
        "standard_seconds",
        "extended_validation_seconds",
        "extended_setup_allowance_seconds",
        "termination_grace_seconds",
        "repositories",
    }
    exact_keys(root, expected, "root")
    raw_repositories = object_mapping(root["repositories"], "repositories")
    if not raw_repositories:
        raise RuntimeError("CI budget policy repositories must not be empty")
    repositories: dict[str, RepositoryPolicy] = {}
    for repository, raw_repository in raw_repositories.items():
        repository_value = object_mapping(raw_repository, f"repositories.{repository}")
        exact_keys(
            repository_value,
            {"costly_paths", "managed_workflows"},
            f"repositories.{repository}",
        )
        repositories[repository] = RepositoryPolicy(
            costly_paths=string_tuple(
                repository_value["costly_paths"],
                f"repositories.{repository}.costly_paths",
            ),
            managed_workflows=string_tuple(
                repository_value["managed_workflows"],
                f"repositories.{repository}.managed_workflows",
            ),
        )
    return Policy(
        big_change_label=non_empty_string(root["big_change_label"], "big_change_label"),
        standard_seconds=positive_int(root["standard_seconds"], "standard_seconds"),
        extended_validation_seconds=positive_int(
            root["extended_validation_seconds"], "extended_validation_seconds"
        ),
        extended_setup_allowance_seconds=positive_int(
            root["extended_setup_allowance_seconds"],
            "extended_setup_allowance_seconds",
        ),
        termination_grace_seconds=positive_int(
            root["termination_grace_seconds"], "termination_grace_seconds"
        ),
        repositories=repositories,
    )


POLICY = load_policy()
STANDARD_BUDGET = timedelta(seconds=POLICY.standard_seconds)


def standard_minutes() -> int:
    return int(STANDARD_BUDGET.total_seconds() // 60)


def worker_timeout_minutes(*, big_change: bool) -> int:
    if big_change:
        return POLICY.extended_worker_minutes
    return standard_minutes()


def repository_policy(repository: str, policy: Policy = POLICY) -> RepositoryPolicy:
    try:
        return policy.repositories[repository]
    except KeyError as error:
        raise RuntimeError(
            f"CI budget policy has no repository entry for {repository!r}"
        ) from error


def classify(
    paths: Sequence[str],
    labels: Sequence[str],
    repository: str,
    *,
    force_big_change: bool,
    policy: Policy = POLICY,
) -> Classification:
    repo_policy = repository_policy(repository, policy)
    matches = [
        {"path": path, "pattern": pattern}
        for path in paths
        for pattern in repo_policy.costly_paths
        if fnmatch.fnmatchcase(path, pattern)
    ]
    sources: list[str] = []
    if force_big_change:
        sources.append("forced")
    if policy.big_change_label in labels:
        sources.append("label")
    if matches:
        sources.append("costly_path")
    return Classification(
        big_change=bool(sources),
        reason={"sources": sources, "matches": matches},
    )


def decide(
    paths: Sequence[str],
    labels: Sequence[str],
    repository: str,
    workflow_path: str,
    *,
    force_big_change: bool,
    policy: Policy = POLICY,
) -> PolicyDecision:
    repo_policy = repository_policy(repository, policy)
    classification = classify(
        paths,
        labels,
        repository,
        force_big_change=force_big_change,
        policy=policy,
    )
    return PolicyDecision(
        classification=classification,
        budget_seconds=(
            policy.extended_validation_seconds + policy.extended_setup_allowance_seconds
            if classification.big_change
            else policy.standard_seconds
        ),
        managed_workflow=workflow_path in repo_policy.managed_workflows,
        standard_seconds=policy.standard_seconds,
        termination_grace_seconds=policy.termination_grace_seconds,
    )


def parse_timestamp(value: object, name: str) -> datetime:
    if not isinstance(value, str):
        raise RuntimeError(f"GitHub API result has no {name}")
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise RuntimeError(f"GitHub API result {name} has no timezone")
    return parsed


def standard_deadline(run: Mapping[str, object]) -> datetime:
    return parse_timestamp(run.get("created_at"), "created_at") + STANDARD_BUDGET


def input_string_list(value: object, name: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item for item in value
    ):
        raise RuntimeError(f"CI budget input {name} must be an array of strings")
    return tuple(value)


def input_bool(value: object, name: str) -> bool:
    if not isinstance(value, bool):
        raise RuntimeError(f"CI budget input {name} must be boolean")
    return value


def cli_decision(payload: object, policy: Policy) -> PolicyDecision:
    value = object_mapping(payload, "input")
    expected = {
        "changed_paths",
        "force_big_change",
        "labels",
        "repository",
        "workflow_path",
    }
    if set(value) != expected:
        raise RuntimeError(
            f"CI budget input keys must be {sorted(expected)}, got {sorted(value)}"
        )
    return decide(
        input_string_list(value["changed_paths"], "changed_paths"),
        input_string_list(value["labels"], "labels"),
        non_empty_string(value["repository"], "input.repository"),
        non_empty_string(value["workflow_path"], "input.workflow_path"),
        force_big_change=input_bool(value["force_big_change"], "force_big_change"),
        policy=policy,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--policy", type=Path, default=POLICY_PATH)
    args = parser.parse_args(argv)
    payload: object = json.load(sys.stdin)
    decision = cli_decision(payload, load_policy(args.policy))
    json.dump(decision.to_json(), sys.stdout, separators=(",", ":"), sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ci-budget-policy: {error}", file=sys.stderr)
        sys.exit(1)
