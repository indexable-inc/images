from __future__ import annotations

import importlib.util
import json
import sys
import unittest
import urllib.parse
import urllib.request
from collections.abc import Mapping
from pathlib import Path
from typing import Any

MODULE_PATH = Path(__file__).with_name("ci_budget.py")
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


class ClassificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.globs = ci_budget.load_canonical_globs(
            MODULE_PATH.with_name("costly-paths.json")
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


class RenderingTests(unittest.TestCase):
    def test_standard_comment_contains_policy_and_budget(self) -> None:
        result = ci_budget.Classification(
            big_change=False, reason={"sources": [], "matches": []}
        )
        comment = ci_budget.render_comment(result)
        assert comment.startswith(ci_budget.COMMENT_MARKER)
        assert ci_budget.COMMENT_POLICY in comment
        assert "Standard budget" in comment


if __name__ == "__main__":
    unittest.main()
