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
HOSTED_CONTROLLER_PATH = Path(".github/workflows/ci-deadline-controller.yml")
OWNER_REFERENCE_PREFIXES = (
    "uses: indexable-inc/index/.github/actions/ci-budget@",
    "uses: indexable-inc/index/.github/actions/ci-budget/worker@",
    "uses: indexable-inc/index/.github/workflows/ci-budget.yml@",
    "uses: indexable-inc/index/.github/workflows/ci-budget-read-only.yml@",
)
CONSUMER_TEXT_SUFFIXES = frozenset(
    {".jq", ".nix", ".py", ".rb", ".sh", ".yaml", ".yml"}
)


@dataclass(frozen=True)
class ConsumerRule:
    name: str
    roots: tuple[str, ...]
    forbidden_tokens: tuple[str, ...]


CONSUMER_FORBIDDEN_PATHS = (".github/workflows/ci-deadline-check.yml",)
CONSUMER_RULES = (
    ConsumerRule(
        name="workflow cancellation",
        roots=(".github/workflows", "nix/checks", "scripts/ci"),
        forbidden_tokens=(
            "CI_BUDGET_DEADLINE_MODE",
            "cancel_at_deadline",
            "cancel_workflow_run",
            "ci-deadline-controller",
            "mode: cancel",
            "/force-cancel",
        ),
    ),
    ConsumerRule(
        name="deadline arithmetic",
        roots=(".github/workflows", "nix/checks", "scripts/ci"),
        forbidden_tokens=(
            "created_at + STANDARD_BUDGET",
            "created_at + budget",
            "created_at + timedelta",
        ),
    ),
)


@dataclass(frozen=True)
class RepositoryPolicy:
    costly_paths: tuple[str, ...]
    managed_workflows: tuple[str, ...]
    merge_queue_branch: str


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
    big_change_label: str
    budget_seconds: int
    managed_workflow: bool
    standard_seconds: int
    termination_grace_seconds: int

    def to_json(self) -> dict[str, object]:
        return {
            "big_change": self.classification.big_change,
            "big_change_label": self.big_change_label,
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
            {"costly_paths", "managed_workflows", "merge_queue_branch"},
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
            merge_queue_branch=non_empty_string(
                repository_value["merge_queue_branch"],
                f"repositories.{repository}.merge_queue_branch",
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
TERMINATION_GRACE = timedelta(seconds=POLICY.termination_grace_seconds)


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
    classification = classify(
        paths,
        labels,
        repository,
        force_big_change=force_big_change,
        policy=policy,
    )
    return decision_from_classification(
        classification,
        repository,
        workflow_path,
        policy=policy,
    )


def decision_from_classification(
    classification: Classification,
    repository: str,
    workflow_path: str,
    *,
    policy: Policy = POLICY,
) -> PolicyDecision:
    repo_policy = repository_policy(repository, policy)
    return PolicyDecision(
        classification=classification,
        big_change_label=policy.big_change_label,
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


def runtime_policy_json(policy: Policy = POLICY) -> dict[str, object]:
    return {
        "schema_version": 1,
        "big_change_label": policy.big_change_label,
        "standard_seconds": policy.standard_seconds,
        "termination_grace_seconds": policy.termination_grace_seconds,
        "repositories": {
            repository: {"managed_workflows": list(repository_policy.managed_workflows)}
            for repository, repository_policy in policy.repositories.items()
        },
    }


def index_input_revision(root: Path) -> str:
    lock_path = root / "flake.lock"
    parsed = object_mapping(json.loads(lock_path.read_text()), "flake.lock")
    nodes = object_mapping(parsed.get("nodes"), "flake.lock.nodes")
    root_name = non_empty_string(parsed.get("root"), "flake.lock.root")
    root_node = object_mapping(nodes.get(root_name), f"flake.lock.nodes.{root_name}")
    inputs = object_mapping(root_node.get("inputs"), "flake.lock root inputs")
    index_name = non_empty_string(inputs.get("index"), "flake.lock root index input")
    index_node = object_mapping(nodes.get(index_name), f"flake.lock.nodes.{index_name}")
    locked = object_mapping(
        index_node.get("locked"), f"flake.lock.nodes.{index_name}.locked"
    )
    if locked.get("owner") != "indexable-inc" or locked.get("repo") != "index":
        raise RuntimeError(
            "CI budget consumer root index input is not indexable-inc/index"
        )
    return non_empty_string(locked.get("rev"), "index locked revision")


def check_owner_revision(root: Path, revision: str) -> None:
    workflows = root / ".github" / "workflows"
    for path in sorted(workflows.glob("*.y*ml")):
        source = path.read_text()
        for line in source.splitlines():
            stripped = line.strip()
            for prefix in OWNER_REFERENCE_PREFIXES:
                if stripped.startswith(prefix) and stripped != f"{prefix}{revision}":
                    raise RuntimeError(
                        f"CI budget owner reference in {path.relative_to(root)} must use "
                        f"the root index input revision {revision}, got {stripped!r}"
                    )


def sync_owner_revision(
    root: Path, repository: str, policy: Policy = POLICY
) -> tuple[Path, ...]:
    repository_policy(repository, policy)
    revision = index_input_revision(root)
    changed = []
    workflows = root / ".github" / "workflows"
    for path in sorted(workflows.glob("*.y*ml")):
        source = path.read_text()
        lines = []
        path_changed = False
        for line in source.splitlines(keepends=True):
            stripped = line.strip()
            matching = [
                prefix
                for prefix in OWNER_REFERENCE_PREFIXES
                if stripped.startswith(prefix)
            ]
            if len(matching) > 1:
                raise RuntimeError(
                    f"CI budget owner reference in {path.relative_to(root)} is ambiguous"
                )
            if matching:
                prefix = matching[0]
                newline = "\n" if line.endswith("\n") else ""
                indentation = line[: len(line) - len(line.lstrip())]
                rendered = f"{indentation}{prefix}{revision}{newline}"
                path_changed |= rendered != line
                lines.append(rendered)
            else:
                lines.append(line)
        if path_changed:
            path.write_text("".join(lines))
            changed.append(path.relative_to(root))
    check_consumer(root, repository, policy)
    return tuple(changed)


def check_consumer_hosted_controller(root: Path, revision: str) -> None:
    path = root / HOSTED_CONTROLLER_PATH
    if not path.exists():
        raise RuntimeError("CI budget consumer has no independent hosted controller")
    source = path.read_text()
    action_prefix = "uses: indexable-inc/index/.github/actions/ci-budget@"
    expected_action = f"{action_prefix}{revision}"
    required = (
        "workflow_run:",
        "types: [requested, in_progress]",
        "runs-on: ubuntu-latest",
        "actions: write",
        "mode: cancel",
        expected_action,
    )
    missing = [token for token in required if token not in source]
    if missing or source.count(action_prefix) != 1:
        detail = f"missing {missing}" if missing else "action reference is duplicated"
        raise RuntimeError(
            f"CI budget hosted controller must be the pinned independent owner consumer: {detail}"
        )


def check_owner_hosted_controller(root: Path) -> None:
    path = root / HOSTED_CONTROLLER_PATH
    if not path.exists():
        raise RuntimeError("CI budget owner has no independent hosted controller")
    source = path.read_text()
    required = (
        "workflows: [Check, Closure gate]",
        "types: [requested, in_progress]",
        "runs-on: ubuntu-latest",
        "actions: write",
        "repository: ${{ job.workflow_repository }}",
        "ref: ${{ job.workflow_sha }}",
        "uses: ./.github/actions/ci-budget",
        "mode: cancel",
    )
    missing = [token for token in required if token not in source]
    if missing or source.count("uses: ./.github/actions/ci-budget") != 1:
        detail = f"missing {missing}" if missing else "action reference is duplicated"
        raise RuntimeError(
            f"CI budget owner controller must use its exact trusted workflow version: {detail}"
        )


def check_consumer(root: Path, repository: str, policy: Policy = POLICY) -> None:
    repository_policy(repository, policy)
    consumer_revision = None
    if repository != "indexable-inc/index":
        consumer_revision = index_input_revision(root)
        check_owner_revision(root, consumer_revision)
    for relative in CONSUMER_FORBIDDEN_PATHS:
        if (root / relative).exists():
            raise RuntimeError(
                f"CI budget consumer {repository} reimplements the dispatcher owner at {relative}"
            )
    for rule in CONSUMER_RULES:
        for relative_root in rule.roots:
            scan_root = root / relative_root
            if not scan_root.exists():
                continue
            for path in sorted(scan_root.rglob("*")):
                if not path.is_file() or path.suffix not in CONSUMER_TEXT_SUFFIXES:
                    continue
                if path == root / HOSTED_CONTROLLER_PATH:
                    continue
                source = path.read_text()
                for token in rule.forbidden_tokens:
                    if token in source:
                        relative = path.relative_to(root)
                        raise RuntimeError(
                            f"CI budget consumer {repository} reimplements {rule.name} "
                            f"in {relative}: found {token!r}"
                        )
    if repository == "indexable-inc/index":
        check_owner_hosted_controller(root)
    else:
        assert consumer_revision is not None
        check_consumer_hosted_controller(root, consumer_revision)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--policy", type=Path, default=POLICY_PATH)
    parser.add_argument("--check-consumer-root", type=Path)
    parser.add_argument("--sync-consumer-root", type=Path)
    parser.add_argument("--repository")
    args = parser.parse_args(argv)
    policy = load_policy(args.policy)
    if args.check_consumer_root is not None and args.sync_consumer_root is not None:
        raise RuntimeError(
            "choose either --check-consumer-root or --sync-consumer-root"
        )
    consumer_root = args.check_consumer_root or args.sync_consumer_root
    if consumer_root is not None:
        if args.repository is None:
            raise RuntimeError("--repository is required with a consumer root")
        changed = ()
        if args.sync_consumer_root is not None:
            changed = sync_owner_revision(consumer_root, args.repository, policy)
        else:
            check_consumer(consumer_root, args.repository, policy)
        json.dump(
            {
                "changed": [str(path) for path in changed],
                "repository": args.repository,
                "status": "ok",
            },
            sys.stdout,
            separators=(",", ":"),
            sort_keys=True,
        )
        sys.stdout.write("\n")
        return 0
    if args.repository is not None:
        raise RuntimeError("--repository requires a consumer root")
    payload: object = json.load(sys.stdin)
    decision = cli_decision(payload, policy)
    json.dump(decision.to_json(), sys.stdout, separators=(",", ":"), sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"ci-budget-policy: {error}", file=sys.stderr)
        sys.exit(1)
