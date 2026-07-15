#!/usr/bin/env python3
"""Classify and publish the shared pull request CI budget."""

from __future__ import annotations

import fnmatch
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

BIG_CHANGE_LABEL = "ci/big-change"
COMMENT_MARKER = "<!-- ci-budget -->"
COMMENT_POLICY = (
    "CI is limited to 5 minutes unless this is a legitimate big change. "
    "Add the `ci/big-change` label for an extended budget. Lockfile and Rust "
    "toolchain changes are labeled automatically."
)

JsonObject = dict[str, Any]
Transport = Callable[[urllib.request.Request], tuple[Any, Mapping[str, str]]]


@dataclass(frozen=True)
class Classification:
    big_change: bool
    reason: JsonObject


def parse_bool(value: str, name: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise ValueError(f"{name} must be 'true' or 'false', got {value!r}")


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
        self._token = token
        self._transport = transport

    def _url(self, path: str, query: Mapping[str, int] | None = None) -> str:
        url = f"https://api.github.com/repos/{self._repository}/{path}"
        if query:
            url = f"{url}?{urllib.parse.urlencode(query)}"
        return url

    def request(
        self,
        method: str,
        path: str,
        body: JsonObject | None = None,
        query: Mapping[str, int] | None = None,
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

    def changed_paths(self, number: int) -> list[str]:
        files = self.paginated(f"pulls/{number}/files")
        paths = [item.get("filename") for item in files]
        if not all(isinstance(path, str) and path for path in paths):
            raise RuntimeError("GitHub API returned a changed file without a filename")
        return paths

    def add_label(self, number: int, label: str) -> None:
        self.request("POST", f"issues/{number}/labels", {"labels": [label]})

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


def render_comment(classification: Classification) -> str:
    if classification.big_change:
        matches = classification.reason["matches"]
        if matches:
            detail = f"Extended budget: matched `{matches[0]['path']}`."
        elif "label" in classification.reason["sources"]:
            detail = "Extended budget: the `ci/big-change` label is present."
        else:
            detail = "Extended budget: this run was explicitly classified as large."
    else:
        detail = "Standard budget: this change has 5 minutes."
    return f"{COMMENT_MARKER}\n{COMMENT_POLICY}\n\n{detail}"


def write_output(name: str, value: str) -> None:
    output_path = os.environ.get("GITHUB_OUTPUT")
    if not output_path:
        raise RuntimeError("GITHUB_OUTPUT is required")
    with Path(output_path).open("a") as output:
        output.write(f"{name}={value}\n")


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
    paths: list[str] = []
    labels: list[str] = []
    if pull_request_number:
        paths = client.changed_paths(pull_request_number)
        labels = labels_from_pull_request(client.pull_request(pull_request_number))

    classification = classify(paths, labels, globs, force_big_change=force_big_change)
    if publish and pull_request_number:
        if classification.reason["matches"] and BIG_CHANGE_LABEL not in labels:
            client.add_label(pull_request_number, BIG_CHANGE_LABEL)
        client.upsert_comment(pull_request_number, render_comment(classification))

    reason = json.dumps(classification.reason, separators=(",", ":"), sort_keys=True)
    write_output("big_change", str(classification.big_change).lower())
    write_output("reason", reason)
    print(reason)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, RuntimeError, ValueError, json.JSONDecodeError) as error:
        print(f"::error title=ci-budget::{error}", file=sys.stderr)
        sys.exit(1)
