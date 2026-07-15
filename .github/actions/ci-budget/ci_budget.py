#!/usr/bin/env python3
"""Classify and publish the shared CI budget."""

from __future__ import annotations

import fnmatch
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ci_policy import standard_deadline, standard_minutes, worker_timeout_minutes

BIG_CHANGE_LABEL = "ci/big-change"
COMMENT_MARKER = "<!-- ci-budget -->"
MAX_PULL_REQUEST_FILES = 3000
CI_BUDGET_SNAPSHOT_PREFIX = "ci-budget-snapshot"
MERGE_QUEUE_ENTRIES_QUERY = """
query MergeQueueEntries(
  $owner: String!
  $name: String!
  $branch: String!
  $after: String
) {
  repository(owner: $owner, name: $name) {
    mergeQueue(branch: $branch) {
      entries(first: 100, after: $after) {
        nodes {
          baseCommit { oid }
          headCommit { oid }
          pullRequest { number }
        }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}
"""
JsonObject = dict[str, Any]
Transport = Callable[[urllib.request.Request], tuple[Any, Mapping[str, str]]]


@dataclass(frozen=True)
class Classification:
    big_change: bool
    reason: JsonObject


@dataclass(frozen=True)
class MergeQueueEdge:
    base_sha: str
    head_sha: str
    pull_request_number: int


@dataclass(frozen=True)
class PushAssociations:
    pull_requests: tuple[JsonObject, ...]
    unassociated_commits: tuple[str, ...]


@dataclass(frozen=True)
class BudgetSnapshot:
    big_change: bool

    def __post_init__(self) -> None:
        if not isinstance(self.big_change, bool):
            raise ValueError("budget snapshot big_change must be boolean")

    @property
    def artifact_key(self) -> str:
        return "extended" if self.big_change else "standard"


def classification_from_snapshot(snapshot: BudgetSnapshot) -> Classification:
    return Classification(
        big_change=snapshot.big_change,
        reason={"sources": ["attempt_snapshot"], "matches": []},
    )


def parse_bool(value: str, name: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise ValueError(f"{name} must be 'true' or 'false', got {value!r}")


def parse_positive_int(value: str, name: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise ValueError(f"{name} must be positive")
    return parsed


def parse_globs(value: str, name: str) -> list[str]:
    decoded = json.loads(value)
    if not isinstance(decoded, list) or not all(
        isinstance(item, str) and item for item in decoded
    ):
        raise ValueError(f"{name} must be a JSON array of non-empty strings")
    return decoded


def load_canonical_globs(path: Path) -> list[str]:
    globs = [line for line in path.read_text().splitlines() if line]
    if not globs:
        raise ValueError(f"{path} must contain at least one glob")
    return globs


def classify(
    paths: Sequence[str],
    labels: Sequence[str],
    globs: Sequence[str],
    *,
    force_big_change: bool,
) -> Classification:
    matches = [
        {"path": path, "pattern": pattern}
        for path in paths
        for pattern in globs
        if fnmatch.fnmatchcase(path, pattern)
    ]
    sources: list[str] = []
    if force_big_change:
        sources.append("forced")
    if BIG_CHANGE_LABEL in labels:
        sources.append("label")
    if matches:
        sources.append("costly_path")
    return Classification(
        big_change=bool(sources),
        reason={"sources": sources, "matches": matches},
    )


def default_transport(request: urllib.request.Request) -> tuple[Any, Mapping[str, str]]:
    if urllib.parse.urlparse(request.full_url).scheme != "https":
        raise ValueError("GitHub API requests must use HTTPS")
    try:
        with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
            body = response.read()
            payload = json.loads(body) if body else None
            return payload, response.headers
    except urllib.error.HTTPError as error:
        detail = error.read().decode(errors="replace")
        raise RuntimeError(
            f"GitHub API {request.method} {request.full_url} failed: "
            f"HTTP {error.code}: {detail}"
        ) from error


class GitHubClient:
    def __init__(
        self,
        repository: str,
        token: str,
        transport: Transport = default_transport,
    ) -> None:
        owner, separator, name = repository.partition("/")
        if not separator or not owner or not name or "/" in name:
            raise ValueError(
                f"repository must have owner/name form, got {repository!r}"
            )
        self._repository = repository
        self._owner = owner
        self._name = name
        self._token = token
        self._transport = transport

    def _url(self, path: str, query: Mapping[str, int | str] | None = None) -> str:
        url = f"https://api.github.com/repos/{self._repository}/{path}"
        if query:
            url = f"{url}?{urllib.parse.urlencode(query)}"
        return url

    def request(
        self,
        method: str,
        path: str,
        body: JsonObject | None = None,
        query: Mapping[str, int | str] | None = None,
    ) -> tuple[Any, Mapping[str, str]]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(  # noqa: S310
            self._url(path, query),
            data=data,
            method=method,
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "Content-Type": "application/json",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        return self._transport(request)

    def graphql(self, query: str, variables: JsonObject) -> JsonObject:
        request = urllib.request.Request(
            "https://api.github.com/graphql",
            data=json.dumps({"query": query, "variables": variables}).encode(),
            method="POST",
            headers={
                "Accept": "application/vnd.github+json",
                "Authorization": f"Bearer {self._token}",
                "Content-Type": "application/json",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        payload, _ = self._transport(request)
        if not isinstance(payload, dict):
            raise RuntimeError("GitHub GraphQL API returned a malformed response")
        errors = payload.get("errors")
        if errors:
            raise RuntimeError(f"GitHub GraphQL API returned errors: {errors!r}")
        data = payload.get("data")
        if not isinstance(data, dict):
            raise RuntimeError("GitHub GraphQL API returned no data")
        return data

    def paginated(self, path: str) -> list[JsonObject]:
        items: list[JsonObject] = []
        page = 1
        while True:
            payload, _ = self.request(
                "GET", path, query={"per_page": 100, "page": page}
            )
            if not isinstance(payload, list):
                raise RuntimeError(f"GitHub API {path} returned a non-list page")
            if not all(isinstance(item, dict) for item in payload):
                raise RuntimeError(f"GitHub API {path} returned a malformed page")
            items.extend(payload)
            if len(payload) < 100:
                return items
            page += 1

    def pull_request(self, number: int) -> JsonObject:
        payload, _ = self.request("GET", f"pulls/{number}")
        if not isinstance(payload, dict):
            raise RuntimeError("GitHub API returned a malformed pull request")
        return payload

    def associated_pull_requests(self, commit_sha: str) -> list[JsonObject]:
        validate_sha(commit_sha, "workflow head SHA")
        return self.paginated(f"commits/{commit_sha}/pulls")

    def main_push_associations(
        self, branch: str, base_sha: str, head_sha: str
    ) -> PushAssociations:
        if not branch:
            raise ValueError("push branch must not be empty")
        validate_sha(base_sha, "push base SHA")
        validate_sha(head_sha, "push head SHA")
        current = head_sha
        seen: set[str] = set()
        pull_requests: list[JsonObject] = []
        unassociated_commits: list[str] = []
        while current != base_sha:
            if current in seen:
                raise RuntimeError("push first-parent range contains a cycle")
            seen.add(current)
            associated = self.associated_pull_requests(current)
            exact = [
                pull_request
                for pull_request in associated
                if pull_request.get("merge_commit_sha") == current
                and isinstance(pull_request.get("merged_at"), str)
                and isinstance(pull_request.get("base"), dict)
                and pull_request["base"].get("ref") == branch
            ]
            if len(exact) > 1:
                raise RuntimeError(
                    f"push commit {current} has {len(exact)} exact merged pull "
                    f"requests for {branch!r}; expected at most one"
                )
            if exact:
                number = pull_request_number(exact[0], "push commit association")
                pull_requests.append(self.pull_request(number))
            else:
                unassociated_commits.append(current)
            parents = self.commit_parents(current)
            if not parents:
                raise RuntimeError(
                    f"push base {base_sha} is not on the first-parent history "
                    f"of {head_sha}"
                )
            current = parents[0]
        return PushAssociations(
            pull_requests=tuple(pull_requests),
            unassociated_commits=tuple(unassociated_commits),
        )

    def commit_parents(self, commit_sha: str) -> list[str]:
        validate_sha(commit_sha, "commit SHA")
        payload, _ = self.request("GET", f"commits/{commit_sha}")
        if not isinstance(payload, dict) or not isinstance(
            payload.get("parents"), list
        ):
            raise RuntimeError("GitHub API returned a commit without parents")
        raw_parents = payload["parents"]
        parents = [item.get("sha") for item in raw_parents if isinstance(item, dict)]
        if len(parents) != len(raw_parents) or not all(
            isinstance(parent, str) for parent in parents
        ):
            raise RuntimeError("GitHub API returned malformed commit parents")
        for parent in parents:
            validate_sha(parent, "commit parent SHA")
        return parents

    def merge_queue_pull_requests(
        self, branch: str, base_sha: str, head_sha: str
    ) -> list[JsonObject]:
        if not branch:
            raise ValueError("merge queue branch must not be empty")
        validate_sha(base_sha, "merge group base SHA")
        validate_sha(head_sha, "merge group head SHA")
        after: str | None = None
        edges: list[MergeQueueEdge] = []
        while True:
            data = self.graphql(
                MERGE_QUEUE_ENTRIES_QUERY,
                {
                    "owner": self._owner,
                    "name": self._name,
                    "branch": branch,
                    "after": after,
                },
            )
            repository = data.get("repository")
            if not isinstance(repository, dict):
                raise RuntimeError("GitHub GraphQL API returned no repository")
            queue = repository.get("mergeQueue")
            if not isinstance(queue, dict):
                raise RuntimeError(
                    f"GitHub GraphQL API returned no merge queue for {branch!r}"
                )
            entries = queue.get("entries")
            if not isinstance(entries, dict):
                raise RuntimeError("GitHub GraphQL API returned no merge queue entries")
            nodes = entries.get("nodes")
            page_info = entries.get("pageInfo")
            if not isinstance(nodes, list) or not all(
                isinstance(node, dict) for node in nodes
            ):
                raise RuntimeError(
                    "GitHub GraphQL API returned malformed queue entries"
                )
            if not isinstance(page_info, dict):
                raise RuntimeError("GitHub GraphQL API returned no queue page info")
            for node in nodes:
                base = node.get("baseCommit")
                head = node.get("headCommit")
                pull_request = node.get("pullRequest")
                if not all(
                    isinstance(item, dict) for item in (base, head, pull_request)
                ):
                    raise RuntimeError("merge queue entry is incomplete")
                edge_base_sha = base.get("oid")
                edge_head_sha = head.get("oid")
                if not isinstance(edge_base_sha, str) or not isinstance(
                    edge_head_sha, str
                ):
                    raise RuntimeError("merge queue entry has malformed commits")
                validate_sha(edge_base_sha, "queue entry base SHA")
                validate_sha(edge_head_sha, "queue entry head SHA")
                edges.append(
                    MergeQueueEdge(
                        base_sha=edge_base_sha,
                        head_sha=edge_head_sha,
                        pull_request_number=pull_request_number(
                            pull_request, "merge queue entry"
                        ),
                    )
                )
            has_next_page = page_info.get("hasNextPage")
            if not isinstance(has_next_page, bool):
                raise RuntimeError("GitHub GraphQL API returned malformed page info")
            if not has_next_page:
                break
            after = page_info.get("endCursor")
            if not isinstance(after, str) or not after:
                raise RuntimeError("GitHub GraphQL API returned no next-page cursor")
        matches = [
            edge
            for edge in edges
            if edge.base_sha == base_sha and edge.head_sha == head_sha
        ]
        if len(matches) != 1:
            raise RuntimeError(
                f"merge group {base_sha}..{head_sha} matches {len(matches)} "
                "queue entries; expected exactly one"
            )
        chain = [matches[0]]
        seen_heads = {head_sha}
        while True:
            predecessors = [
                edge for edge in edges if edge.head_sha == chain[-1].base_sha
            ]
            if not predecessors:
                break
            if len(predecessors) != 1:
                raise RuntimeError(
                    f"merge group predecessor {chain[-1].base_sha} matches "
                    f"{len(predecessors)} queue entries; expected at most one"
                )
            predecessor = predecessors[0]
            if predecessor.head_sha in seen_heads:
                raise RuntimeError("merge queue entry chain contains a cycle")
            seen_heads.add(predecessor.head_sha)
            chain.append(predecessor)
        return [self.pull_request(edge.pull_request_number) for edge in chain]

    def changed_paths(self, number: int, expected_count: int) -> list[str]:
        if expected_count < 0:
            raise RuntimeError("pull request changed_files must not be negative")
        if expected_count > MAX_PULL_REQUEST_FILES:
            raise RuntimeError(
                f"pull request {number} has {expected_count} changed files; "
                f"GitHub exposes at most {MAX_PULL_REQUEST_FILES}"
            )
        files: list[JsonObject] = []
        page = 1
        while len(files) < expected_count:
            payload, _ = self.request(
                "GET",
                f"pulls/{number}/files",
                query={"per_page": 100, "page": page},
            )
            if not isinstance(payload, list) or not all(
                isinstance(item, dict) for item in payload
            ):
                raise RuntimeError("GitHub API returned malformed changed files")
            files.extend(payload)
            if len(payload) < 100:
                break
            page += 1
        if len(files) != expected_count:
            raise RuntimeError(
                f"pull request {number} reports {expected_count} changed files "
                f"but GitHub returned {len(files)}"
            )
        paths = [item.get("filename") for item in files]
        if not all(isinstance(path, str) and path for path in paths):
            raise RuntimeError("GitHub API returned a changed file without a filename")
        if len(set(paths)) != len(paths):
            raise RuntimeError("GitHub API returned duplicate changed filenames")
        return paths

    def ci_budget_snapshot(
        self, run_id: int, run_attempt: int
    ) -> BudgetSnapshot | None:
        prefix = f"{CI_BUDGET_SNAPSHOT_PREFIX}-{run_id}-"
        artifacts: list[JsonObject] = []
        page = 1
        while True:
            payload, _ = self.request(
                "GET",
                f"actions/runs/{run_id}/artifacts",
                query={"per_page": 100, "page": page},
            )
            if not isinstance(payload, dict) or not isinstance(
                payload.get("artifacts"), list
            ):
                raise RuntimeError("GitHub API returned malformed workflow artifacts")
            page_artifacts = payload["artifacts"]
            if not all(isinstance(artifact, dict) for artifact in page_artifacts):
                raise RuntimeError("GitHub API returned a malformed workflow artifact")
            artifacts.extend(page_artifacts)
            if len(page_artifacts) < 100:
                break
            page += 1
        snapshots: list[tuple[int, BudgetSnapshot]] = []
        pattern = re.compile(
            rf"{re.escape(prefix)}(?P<attempt>[1-9][0-9]*)-"
            r"(?P<tier>standard|extended)"
        )
        for artifact in artifacts:
            name = artifact.get("name")
            if not isinstance(name, str) or artifact.get("expired") is not False:
                continue
            match = pattern.fullmatch(name)
            if match is None:
                continue
            artifact_attempt = int(match.group("attempt"))
            if artifact_attempt <= run_attempt:
                snapshot = BudgetSnapshot(big_change=match.group("tier") == "extended")
                snapshots.append((artifact_attempt, snapshot))
        exact = [snapshot for attempt, snapshot in snapshots if attempt == run_attempt]
        if len(exact) > 1:
            raise RuntimeError(
                f"workflow attempt has {len(exact)} CI budget snapshot artifacts; "
                "expected exactly one"
            )
        if exact:
            return exact[0]
        inherited = {
            snapshot for attempt, snapshot in snapshots if attempt < run_attempt
        }
        if not inherited:
            return None
        if len(inherited) != 1:
            raise RuntimeError(
                "earlier workflow attempts disagree on the CI budget snapshot"
            )
        return inherited.pop()

    def add_label(self, number: int, label: str) -> None:
        self.request("POST", f"issues/{number}/labels", {"labels": [label]})

    def workflow_attempt(self, run_id: int, run_attempt: int) -> JsonObject:
        payload, _ = self.request(
            "GET", f"actions/runs/{run_id}/attempts/{run_attempt}"
        )
        if not isinstance(payload, dict):
            raise RuntimeError("GitHub API returned a malformed workflow attempt")
        return payload

    def workflow_jobs(self, run_id: int, run_attempt: int) -> list[JsonObject]:
        jobs: list[JsonObject] = []
        page = 1
        while True:
            payload, _ = self.request(
                "GET",
                f"actions/runs/{run_id}/attempts/{run_attempt}/jobs",
                query={"per_page": 100, "page": page},
            )
            if not isinstance(payload, dict) or not isinstance(
                payload.get("jobs"), list
            ):
                raise RuntimeError("GitHub API returned malformed workflow jobs")
            page_jobs = payload["jobs"]
            if not all(isinstance(job, dict) for job in page_jobs):
                raise RuntimeError("GitHub API returned a malformed workflow job")
            jobs.extend(page_jobs)
            if len(page_jobs) < 100:
                return jobs
            page += 1

    def upsert_comment(self, number: int, body: str) -> None:
        comments = self.paginated(f"issues/{number}/comments")
        owned = [
            item
            for item in comments
            if isinstance(item.get("body"), str)
            and item["body"].startswith(COMMENT_MARKER)
            and isinstance(item.get("user"), dict)
            and item["user"].get("login") in {"github-actions", "github-actions[bot]"}
        ]
        if len(owned) > 1:
            raise RuntimeError(
                "multiple CI budget comments exist; refusing an ambiguous update"
            )
        if owned:
            comment_id = owned[0].get("id")
            if not isinstance(comment_id, int):
                raise RuntimeError("existing CI budget comment has no numeric id")
            self.request("PATCH", f"issues/comments/{comment_id}", {"body": body})
        else:
            self.request("POST", f"issues/{number}/comments", {"body": body})


def labels_from_pull_request(pull_request: JsonObject) -> list[str]:
    raw = pull_request.get("labels")
    if not isinstance(raw, list):
        raise RuntimeError("GitHub API returned a pull request without labels")
    labels = [item.get("name") for item in raw if isinstance(item, dict)]
    if len(labels) != len(raw) or not all(isinstance(label, str) for label in labels):
        raise RuntimeError("GitHub API returned a malformed pull request label")
    return labels


def validate_sha(value: str, name: str) -> None:
    if not re.fullmatch(r"[0-9a-f]{40}", value):
        raise ValueError(f"{name} must be a lowercase 40-character SHA")


def pull_request_number(pull_request: JsonObject, source: str) -> int:
    number = pull_request.get("number")
    if not isinstance(number, int) or number <= 0:
        raise RuntimeError(f"{source} has no positive pull request number")
    return number


def pull_requests_for_attempt(
    client: GitHubClient,
    attempt: JsonObject,
    *,
    merge_queue_branch: str,
    event_base_sha: str | None,
    event_pull_request_number: int | None,
) -> list[JsonObject]:
    event = attempt.get("event")
    head_sha = attempt.get("head_sha")
    if event in {"merge_group", "push"} and not isinstance(head_sha, str):
        raise RuntimeError("GitHub API workflow attempt has no head SHA")
    if event == "merge_group":
        assert isinstance(head_sha, str)
        if event_base_sha is None:
            raise RuntimeError("merge group classification requires its base SHA")
        return client.merge_queue_pull_requests(
            merge_queue_branch, event_base_sha, head_sha
        )

    raw_pull_requests = attempt.get("pull_requests")
    if not isinstance(raw_pull_requests, list) or not all(
        isinstance(item, dict) for item in raw_pull_requests
    ):
        raise RuntimeError("GitHub API workflow attempt has malformed pull requests")
    if len(raw_pull_requests) > 1:
        raise RuntimeError("workflow attempt identifies multiple pull requests")
    if event_pull_request_number is not None:
        if raw_pull_requests:
            payload_number = pull_request_number(
                raw_pull_requests[0], "workflow attempt"
            )
            if payload_number != event_pull_request_number:
                raise RuntimeError(
                    "workflow attempt pull request disagrees with source context"
                )
        return [client.pull_request(event_pull_request_number)]
    if raw_pull_requests:
        number = pull_request_number(raw_pull_requests[0], "workflow attempt")
        return [client.pull_request(number)]

    raise RuntimeError(f"{event!r} workflow attempt identifies no pull request")


def classify_workflow_attempt(
    client: GitHubClient,
    attempt: JsonObject,
    globs: Sequence[str],
    *,
    force_big_change: bool,
    merge_queue_branch: str,
    event_base_sha: str | None = None,
    event_pull_request_number: int | None = None,
) -> Classification:
    if force_big_change:
        return classify([], [], globs, force_big_change=True)
    if attempt.get("event") == "push":
        head_sha = attempt.get("head_sha")
        if not isinstance(head_sha, str):
            raise RuntimeError("GitHub API workflow attempt has no head SHA")
        if event_base_sha is None:
            raise RuntimeError("push workflow classification requires its base SHA")
        associations = client.main_push_associations(
            merge_queue_branch, event_base_sha, head_sha
        )
        if associations.unassociated_commits:
            return Classification(
                big_change=True,
                reason={
                    "sources": ["unassociated_push_commit"],
                    "matches": [
                        {"commit": commit}
                        for commit in associations.unassociated_commits
                    ],
                },
            )
        return classify_pull_requests(
            client,
            associations.pull_requests,
            globs,
            force_big_change=False,
        )
    pull_requests = pull_requests_for_attempt(
        client,
        attempt,
        merge_queue_branch=merge_queue_branch,
        event_base_sha=event_base_sha,
        event_pull_request_number=event_pull_request_number,
    )
    return classify_pull_requests(client, pull_requests, globs, force_big_change=False)


def classify_pull_request(
    client: GitHubClient,
    pull_request: JsonObject,
    globs: Sequence[str],
    *,
    force_big_change: bool,
) -> Classification:
    return classify_pull_requests(
        client, [pull_request], globs, force_big_change=force_big_change
    )


def classify_pull_requests(
    client: GitHubClient,
    pull_requests: Sequence[JsonObject],
    globs: Sequence[str],
    *,
    force_big_change: bool,
) -> Classification:
    if not pull_requests:
        raise RuntimeError("classification requires at least one pull request")
    labels: list[str] = []
    for pull_request in pull_requests:
        labels.extend(labels_from_pull_request(pull_request))
    labels = list(dict.fromkeys(labels))
    if force_big_change or BIG_CHANGE_LABEL in labels:
        return classify([], labels, globs, force_big_change=force_big_change)
    paths: list[str] = []
    for pull_request in pull_requests:
        number = pull_request_number(pull_request, "pull request")
        changed_files = pull_request.get("changed_files")
        if not isinstance(changed_files, int):
            raise RuntimeError(
                "GitHub API returned a pull request without changed_files"
            )
        paths.extend(client.changed_paths(number, changed_files))
    return classify(
        list(dict.fromkeys(paths)),
        labels,
        globs,
        force_big_change=force_big_change,
    )


def render_comment(classification: Classification) -> str:
    minutes = standard_minutes()
    if classification.big_change:
        matches = classification.reason["matches"]
        if matches:
            detail = f"Extended budget: matched `{matches[0]['path']}`."
        elif "label" in classification.reason["sources"]:
            detail = "Extended budget: the `ci/big-change` label is present."
        else:
            detail = "Extended budget: this run was explicitly classified as large."
    else:
        detail = f"Standard budget: this change has {minutes} minutes."
    policy_text = (
        f"CI is limited to {minutes} minutes unless this is a legitimate "
        "big change. Add the `ci/big-change` label for an extended budget. "
        "Lockfile and Rust toolchain changes are labeled automatically."
    )
    return f"{COMMENT_MARKER}\n{policy_text}\n\n{detail}"


def write_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        raise RuntimeError("GITHUB_OUTPUT is required")
    with Path(output_path).open("a") as output:
        output.write(f"{name}={value}\n")


def write_snapshot(
    path: Path, classification: Classification, head_sha: str
) -> BudgetSnapshot:
    validate_sha(head_sha, "classification head SHA")
    snapshot = BudgetSnapshot(big_change=classification.big_change)
    path.write_text(
        json.dumps(
            {
                "big_change": snapshot.big_change,
                "head_sha": head_sha,
                "reason": classification.reason,
            },
            separators=(",", ":"),
            sort_keys=True,
        )
        + "\n"
    )
    return snapshot


def main() -> int:
    repository = os.environ["CI_BUDGET_REPOSITORY"]
    token = os.environ["CI_BUDGET_TOKEN"]
    pull_request_number = int(os.environ["CI_BUDGET_PULL_REQUEST_NUMBER"])
    if pull_request_number < 0:
        raise ValueError("pull request number must not be negative")
    force_big_change = parse_bool(
        os.environ["CI_BUDGET_FORCE_BIG_CHANGE"], "force-big-change"
    )
    publish = parse_bool(os.environ["CI_BUDGET_PUBLISH"], "publish")
    globs = load_canonical_globs(Path(__file__).with_name("costly-paths"))
    globs.extend(
        parse_globs(os.environ["CI_BUDGET_EXTRA_COSTLY_PATHS"], "extra-costly-paths")
    )

    client = GitHubClient(repository, token)
    run_id = parse_positive_int(os.environ["CI_BUDGET_RUN_ID"], "run-id")
    run_attempt = parse_positive_int(os.environ["CI_BUDGET_RUN_ATTEMPT"], "run-attempt")
    attempt = client.workflow_attempt(run_id, run_attempt)
    labels: list[str] = []
    if run_attempt > 1:
        snapshot = client.ci_budget_snapshot(run_id, run_attempt)
        if snapshot is None:
            raise RuntimeError("workflow retry has no earlier CI budget snapshot")
        classification = classification_from_snapshot(snapshot)
        # The attempt-one publisher owns the human-facing reason. A retry must
        # consume that frozen tier without rewriting it from live PR state.
        publish = False
    elif pull_request_number:
        pull_request = client.pull_request(pull_request_number)
        labels = labels_from_pull_request(pull_request)
        classification = classify_pull_request(
            client,
            pull_request,
            globs,
            force_big_change=force_big_change,
        )
    else:
        head_sha = os.environ["CI_BUDGET_HEAD_SHA"]
        if head_sha:
            if attempt.get("head_sha") != head_sha:
                raise RuntimeError(
                    "workflow attempt head SHA does not match classification head-sha"
                )
            classification = classify_workflow_attempt(
                client,
                attempt,
                globs,
                force_big_change=force_big_change,
                merge_queue_branch=os.environ["CI_BUDGET_MERGE_QUEUE_BRANCH"],
                event_base_sha=os.environ["CI_BUDGET_BASE_SHA"] or None,
            )
        elif force_big_change:
            classification = classify([], [], globs, force_big_change=True)
        else:
            raise ValueError("a routine non-pull-request change requires head-sha")
    if publish and pull_request_number:
        if classification.reason["matches"] and BIG_CHANGE_LABEL not in labels:
            client.add_label(pull_request_number, BIG_CHANGE_LABEL)
        client.upsert_comment(pull_request_number, render_comment(classification))

    reason = json.dumps(classification.reason, separators=(",", ":"), sort_keys=True)
    write_output("big_change", str(classification.big_change).lower())
    write_output("reason", reason)
    write_output("standard_minutes", str(standard_minutes()))
    write_output("standard_deadline", standard_deadline(attempt).isoformat())
    write_output(
        "worker_timeout_minutes",
        str(worker_timeout_minutes(big_change=classification.big_change)),
    )
    attempt_head_sha = attempt.get("head_sha")
    if not isinstance(attempt_head_sha, str):
        raise RuntimeError("GitHub API workflow attempt has no head SHA")
    snapshot_file = Path(os.environ["CI_BUDGET_SNAPSHOT_PATH"])
    snapshot = write_snapshot(snapshot_file, classification, attempt_head_sha)
    write_output("snapshot_path", str(snapshot_file))
    write_output("snapshot_key", snapshot.artifact_key)
    print(reason)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-budget::{error}", file=sys.stderr)
        sys.exit(1)
