from __future__ import annotations

import importlib.util
import json
import sys
import unittest
import urllib.parse
import urllib.request
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("ci_budget.py")
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("ci_budget", MODULE_PATH)
assert SPEC is not None
assert SPEC.loader is not None
ci_budget = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = ci_budget
SPEC.loader.exec_module(ci_budget)


class FakeTransport:
    def __init__(self, responses: list[Any]) -> None:
        self.responses = responses
        self.requests: list[urllib.request.Request] = []

    def __call__(
        self, request: urllib.request.Request
    ) -> tuple[Any, Mapping[str, str]]:
        self.requests.append(request)
        return self.responses.pop(0), {}


def runtime_error(call: Callable[[], object]) -> RuntimeError:
    try:
        call()
    except RuntimeError as error:
        return error
    raise AssertionError("call did not raise RuntimeError")


class ClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.globs = ci_budget.load_canonical_globs(
            MODULE_PATH.with_name("costly-paths")
        )

    def test_routine_change_uses_standard_budget(self) -> None:
        result = ci_budget.classify(
            ["src/main.rs"], [], self.globs, force_big_change=False
        )
        assert not result.big_change
        assert result.reason == {"sources": [], "matches": []}

    def test_root_and_nested_costly_paths_use_extended_budget(self) -> None:
        result = ci_budget.classify(
            ["flake.lock", "guest/rust-toolchain.toml", "vendor/Cargo.lock"],
            [],
            self.globs,
            force_big_change=False,
        )
        assert result.big_change
        assert result.reason["sources"] == ["costly_path"]
        assert {match["path"] for match in result.reason["matches"]} == {
            "flake.lock",
            "guest/rust-toolchain.toml",
            "vendor/Cargo.lock",
        }

    def test_label_and_extra_glob_are_structured_sources(self) -> None:
        result = ci_budget.classify(
            ["images/base.nix"],
            [ci_budget.BIG_CHANGE_LABEL],
            ["images/**"],
            force_big_change=False,
        )
        assert result.reason["sources"] == ["label", "costly_path"]
        assert result.reason["matches"] == [
            {"path": "images/base.nix", "pattern": "images/**"}
        ]

    def test_forced_run_uses_extended_budget(self) -> None:
        result = ci_budget.classify([], [], self.globs, force_big_change=True)
        assert result.big_change
        assert result.reason["sources"] == ["forced"]


class GitHubClientTests(unittest.TestCase):
    def test_changed_files_are_paginated(self) -> None:
        first = [{"filename": f"src/{index}.rs"} for index in range(100)]
        transport = FakeTransport([first, [{"filename": "flake.lock"}]])
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        paths = client.changed_paths(42)

        assert len(paths) == 101
        assert paths[-1] == "flake.lock"
        queries = [
            urllib.parse.parse_qs(urllib.parse.urlparse(request.full_url).query)
            for request in transport.requests
        ]
        assert queries[0]["page"] == ["1"]
        assert queries[1]["page"] == ["2"]

    def test_owned_sticky_comment_is_updated(self) -> None:
        transport = FakeTransport(
            [
                [
                    {
                        "id": 7,
                        "body": f"{ci_budget.COMMENT_MARKER}\nold",
                        "user": {"login": "github-actions[bot]"},
                    }
                ],
                None,
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        client.upsert_comment(42, "new")

        request = transport.requests[-1]
        assert request.method == "PATCH"
        assert request.full_url.endswith("/issues/comments/7")
        assert json.loads(request.data or b"{}") == {"body": "new"}

    def test_user_marker_is_not_claimed(self) -> None:
        transport = FakeTransport(
            [
                [
                    {
                        "id": 7,
                        "body": f"{ci_budget.COMMENT_MARKER}\nspoof",
                        "user": {"login": "someone"},
                    }
                ],
                None,
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        client.upsert_comment(42, "new")

        assert transport.requests[-1].method == "POST"
        assert transport.requests[-1].full_url.endswith("/issues/42/comments")

    def test_merge_queue_entry_is_matched_by_base_and_head(self) -> None:
        transport = FakeTransport(
            [
                {
                    "data": {
                        "repository": {
                            "mergeQueue": {
                                "entries": {
                                    "nodes": [
                                        {
                                            "baseCommit": {"oid": "a" * 40},
                                            "headCommit": {"oid": "b" * 40},
                                            "pullRequest": {"number": 42},
                                        }
                                    ],
                                    "pageInfo": {
                                        "hasNextPage": False,
                                        "endCursor": None,
                                    },
                                }
                            }
                        }
                    }
                },
                {"number": 42, "labels": [{"name": ci_budget.BIG_CHANGE_LABEL}]},
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        pull_requests = client.merge_queue_pull_requests("main", "a" * 40, "b" * 40)

        assert [pull_request["number"] for pull_request in pull_requests] == [42]
        request = transport.requests[0]
        assert request.full_url == "https://api.github.com/graphql"
        body = json.loads(request.data or b"{}")
        assert body["variables"] == {
            "after": None,
            "branch": "main",
            "name": "index",
            "owner": "indexable-inc",
        }

    def test_merge_queue_association_fails_when_entry_is_missing(self) -> None:
        transport = FakeTransport(
            [
                {
                    "data": {
                        "repository": {
                            "mergeQueue": {
                                "entries": {
                                    "nodes": [],
                                    "pageInfo": {
                                        "hasNextPage": False,
                                        "endCursor": None,
                                    },
                                }
                            }
                        }
                    }
                }
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        error = runtime_error(
            lambda: client.merge_queue_pull_requests("main", "a" * 40, "b" * 40)
        )

        assert "matches 0 queue entries" in str(error)

    def test_merge_queue_chain_includes_every_pull_request(self) -> None:
        transport = FakeTransport(
            [
                {
                    "data": {
                        "repository": {
                            "mergeQueue": {
                                "entries": {
                                    "nodes": [
                                        {
                                            "baseCommit": {"oid": "a" * 40},
                                            "headCommit": {"oid": "b" * 40},
                                            "pullRequest": {"number": 41},
                                        },
                                        {
                                            "baseCommit": {"oid": "b" * 40},
                                            "headCommit": {"oid": "c" * 40},
                                            "pullRequest": {"number": 42},
                                        },
                                    ],
                                    "pageInfo": {
                                        "hasNextPage": False,
                                        "endCursor": None,
                                    },
                                }
                            }
                        }
                    }
                },
                {"number": 42, "labels": []},
                {"number": 41, "labels": []},
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)

        pull_requests = client.merge_queue_pull_requests("main", "b" * 40, "c" * 40)

        assert [pull_request["number"] for pull_request in pull_requests] == [42, 41]


class WorkflowAssociationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.globs = ci_budget.load_canonical_globs(
            MODULE_PATH.with_name("costly-paths")
        )

    def test_labeled_main_push_uses_associated_pull_request(self) -> None:
        transport = FakeTransport(
            [
                [
                    {
                        "number": 42,
                        "base": {"ref": "main"},
                        "labels": [{"name": ci_budget.BIG_CHANGE_LABEL}],
                        "merge_commit_sha": "b" * 40,
                        "merged_at": "2026-07-15T10:00:00Z",
                    }
                ],
                [{"filename": "src/main.rs"}],
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)
        attempt = {
            "event": "push",
            "head_sha": "b" * 40,
            "pull_requests": [],
        }

        result = ci_budget.classify_workflow_attempt(
            client,
            attempt,
            self.globs,
            force_big_change=False,
            merge_queue_branch="main",
        )

        assert result.big_change
        assert result.reason["sources"] == ["label"]

    def test_labeled_merge_group_uses_current_queue_entry(self) -> None:
        transport = FakeTransport(
            [
                {"parents": [{"sha": "a" * 40}]},
                {
                    "data": {
                        "repository": {
                            "mergeQueue": {
                                "entries": {
                                    "nodes": [
                                        {
                                            "baseCommit": {"oid": "a" * 40},
                                            "headCommit": {"oid": "b" * 40},
                                            "pullRequest": {"number": 42},
                                        }
                                    ],
                                    "pageInfo": {
                                        "hasNextPage": False,
                                        "endCursor": None,
                                    },
                                }
                            }
                        }
                    }
                },
                {"number": 42, "labels": [{"name": ci_budget.BIG_CHANGE_LABEL}]},
                [{"filename": "src/main.rs"}],
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)
        attempt = {
            "event": "merge_group",
            "head_sha": "b" * 40,
            "pull_requests": [],
        }

        result = ci_budget.classify_workflow_attempt(
            client,
            attempt,
            self.globs,
            force_big_change=False,
            merge_queue_branch="main",
        )

        assert result.big_change
        assert result.reason["sources"] == ["label"]

    def test_ambiguous_main_push_association_fails_loudly(self) -> None:
        transport = FakeTransport(
            [
                [
                    {
                        "number": 41,
                        "base": {"ref": "main"},
                        "labels": [],
                        "merge_commit_sha": "b" * 40,
                        "merged_at": "2026-07-15T10:00:00Z",
                    },
                    {
                        "number": 42,
                        "base": {"ref": "main"},
                        "labels": [],
                        "merge_commit_sha": "b" * 40,
                        "merged_at": "2026-07-15T10:00:00Z",
                    },
                ]
            ]
        )
        client = ci_budget.GitHubClient("indexable-inc/index", "token", transport)
        attempt = {
            "event": "push",
            "head_sha": "b" * 40,
            "pull_requests": [],
        }

        error = runtime_error(
            lambda: ci_budget.classify_workflow_attempt(
                client,
                attempt,
                self.globs,
                force_big_change=False,
                merge_queue_branch="main",
            )
        )

        assert "has 2 exact merged pull requests" in str(error)


class RenderingTests(unittest.TestCase):
    def test_standard_comment_contains_policy_and_budget(self) -> None:
        result = ci_budget.Classification(
            big_change=False, reason={"sources": [], "matches": []}
        )
        comment = ci_budget.render_comment(result)
        assert comment.startswith(ci_budget.COMMENT_MARKER)
        assert "CI is limited to 5 minutes" in comment
        assert "Standard budget" in comment


if __name__ == "__main__":
    unittest.main()
