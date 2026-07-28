from __future__ import annotations

import argparse
import fnmatch
import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime
from math import ceil
from pathlib import Path

POLICY_PATH = Path(__file__).with_name("catalog") / "policy.json"


@dataclass(frozen=True)
class RepositoryPolicy:
    costly_paths: tuple[str, ...]
    managed_workflows: tuple[str, ...]
    # A repository's own measured routine budget. What a gate costs is a
    # property of that repository's check surface and store warmth, not of this
    # policy, so one shared number cannot be right for two repositories at once:
    # ix's gate passed in 5012s at its slowest while the shared default was
    # 300s. None falls back to the catalog default, which exists only for a
    # repository nobody has measured yet.
    routine_validation_seconds: int | None


@dataclass(frozen=True)
class Policy:
    big_change_label: str
    queue_start_seconds: int
    setup_allowance_seconds: int
    routine_validation_seconds: int
    extended_validation_seconds: int
    termination_grace_seconds: int
    repositories: Mapping[str, RepositoryPolicy]

    def routine_seconds(self, repository: str) -> int:
        override = repository_policy(repository, self).routine_validation_seconds
        if override is None:
            return self.routine_validation_seconds
        return override

    def validation_seconds(self, repository: str, *, big_change: bool) -> int:
        if big_change:
            return self.extended_validation_seconds
        return self.routine_seconds(repository)

    def worker_timeout_minutes(self, repository: str, *, big_change: bool) -> int:
        seconds = (
            self.setup_allowance_seconds
            + self.validation_seconds(repository, big_change=big_change)
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
    managed_workflow: bool
    queue_start_seconds: int
    setup_allowance_seconds: int
    validation_seconds: int
    termination_grace_seconds: int
    worker_timeout_minutes: int

    def to_json(self) -> dict[str, object]:
        return {
            "big_change": self.classification.big_change,
            "managed_workflow": self.managed_workflow,
            "queue_start_seconds": self.queue_start_seconds,
            "reason": self.classification.reason,
            "setup_allowance_seconds": self.setup_allowance_seconds,
            "termination_grace_seconds": self.termination_grace_seconds,
            "validation_seconds": self.validation_seconds,
            "worker_timeout_minutes": self.worker_timeout_minutes,
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


ROOT_THRESHOLDS = (
    "extended_validation_seconds",
    "queue_start_seconds",
    "routine_validation_seconds",
    "setup_allowance_seconds",
    "termination_grace_seconds",
)


def check_notes(value: object, thresholds: Sequence[str]) -> None:
    """Every threshold in the catalog must carry its justification next to it.

    A number in a JSON catalog cannot hold a comment, and an undocumented
    threshold is how 300s survived long enough to kill 54.5% of the ix gates
    that went on to pass (indexable-inc/ix#8688). Requiring the note key set to
    equal the threshold set exactly means a threshold cannot be added, renamed,
    or removed without its evidence moving with it. The worker's own JavaScript
    loader accepts `notes` and ignores it: this is a catalog lint, checked
    wherever `load_policy` runs, not a runtime invariant of the kill path.
    """
    notes = object_mapping(value, "notes")
    for name, note in notes.items():
        non_empty_string(note, f"notes.{name}")
    exact_keys(notes, set(thresholds), "notes")


def load_policy(path: Path = POLICY_PATH) -> Policy:
    parsed: object = json.loads(path.read_text())
    root = object_mapping(parsed, "root")
    expected = {
        "big_change_label",
        "notes",
        "queue_start_seconds",
        "setup_allowance_seconds",
        "routine_validation_seconds",
        "extended_validation_seconds",
        "termination_grace_seconds",
        "repositories",
    }
    exact_keys(root, expected, "root")
    raw_repositories = object_mapping(root["repositories"], "repositories")
    if not raw_repositories:
        raise RuntimeError("CI budget policy repositories must not be empty")
    repositories: dict[str, RepositoryPolicy] = {}
    documented: list[str] = list(ROOT_THRESHOLDS)
    for repository, raw_repository in raw_repositories.items():
        repository_value = object_mapping(raw_repository, f"repositories.{repository}")
        required = {"costly_paths", "managed_workflows"}
        optional = {"routine_validation_seconds"}
        if not required <= set(repository_value) <= required | optional:
            raise RuntimeError(
                f"CI budget policy repositories.{repository} keys must be "
                f"{sorted(required)} plus at most {sorted(optional)}, "
                f"got {sorted(repository_value)}"
            )
        routine_override: int | None = None
        if "routine_validation_seconds" in repository_value:
            routine_override = positive_int(
                repository_value["routine_validation_seconds"],
                f"repositories.{repository}.routine_validation_seconds",
            )
            documented.append(f"repositories.{repository}.routine_validation_seconds")
        repositories[repository] = RepositoryPolicy(
            costly_paths=string_tuple(
                repository_value["costly_paths"],
                f"repositories.{repository}.costly_paths",
            ),
            managed_workflows=string_tuple(
                repository_value["managed_workflows"],
                f"repositories.{repository}.managed_workflows",
            ),
            routine_validation_seconds=routine_override,
        )
    check_notes(root["notes"], documented)
    return Policy(
        big_change_label=non_empty_string(root["big_change_label"], "big_change_label"),
        queue_start_seconds=positive_int(
            root["queue_start_seconds"], "queue_start_seconds"
        ),
        setup_allowance_seconds=positive_int(
            root["setup_allowance_seconds"], "setup_allowance_seconds"
        ),
        routine_validation_seconds=positive_int(
            root["routine_validation_seconds"], "routine_validation_seconds"
        ),
        extended_validation_seconds=positive_int(
            root["extended_validation_seconds"], "extended_validation_seconds"
        ),
        termination_grace_seconds=positive_int(
            root["termination_grace_seconds"], "termination_grace_seconds"
        ),
        repositories=repositories,
    )


POLICY = load_policy()


def queue_start_minutes() -> int:
    return ceil(POLICY.queue_start_seconds / 60)


def validation_seconds(repository: str, *, big_change: bool) -> int:
    return POLICY.validation_seconds(repository, big_change=big_change)


def worker_timeout_minutes(repository: str, *, big_change: bool) -> int:
    return POLICY.worker_timeout_minutes(repository, big_change=big_change)


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
        managed_workflow=workflow_path in repo_policy.managed_workflows,
        queue_start_seconds=policy.queue_start_seconds,
        setup_allowance_seconds=policy.setup_allowance_seconds,
        validation_seconds=policy.validation_seconds(
            repository, big_change=classification.big_change
        ),
        termination_grace_seconds=policy.termination_grace_seconds,
        worker_timeout_minutes=policy.worker_timeout_minutes(
            repository, big_change=classification.big_change
        ),
    )


def parse_timestamp(value: object, name: str) -> datetime:
    if not isinstance(value, str):
        raise RuntimeError(f"GitHub API result has no {name}")
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        raise RuntimeError(f"GitHub API result {name} has no timezone")
    return parsed


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
