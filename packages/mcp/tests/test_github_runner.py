"""Regression tests for organization-scoped Actions runner lookup (#3041)."""

from __future__ import annotations

import asyncio

import pytest

import github


def _page(*runners: dict[str, object]) -> list[dict[str, object]]:
    return [{"total_count": len(runners), "runners": list(runners)}]


def _runner(name: str) -> dict[str, object]:
    return {
        "id": 89805,
        "name": name,
        "status": "online",
        "busy": True,
        "labels": [{"id": 1, "name": "ix-ci"}],
    }


def test_runner_finds_organization_scope_after_empty_repository(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    target = "ix-ci-job-ix-29211743316-86700551213"
    requests: list[tuple[str, ...]] = []

    async def request(*args: str) -> object:
        requests.append(args)
        endpoint = args[-1]
        if endpoint == "repos/indexable-inc/ix":
            return {"owner": {"login": "indexable-inc", "type": "Organization"}}
        if endpoint.startswith("repos/indexable-inc/ix/actions/runners"):
            return _page()
        if endpoint.startswith("orgs/indexable-inc/actions/runners"):
            return [*_page(), *_page(_runner(target))]
        raise AssertionError(endpoint)

    monkeypatch.setattr(github, "_request_json", request)
    result = asyncio.run(github.runner("indexable-inc/ix", target))

    assert result.scopes_checked == ["repository", "organization"]
    assert [(match.scope, match.status, match.busy) for match in result.matches] == [
        ("organization", "online", True)
    ]
    assert {request[-1].split("?", 1)[0] for request in requests[1:]} == {
        "repos/indexable-inc/ix/actions/runners",
        "orgs/indexable-inc/actions/runners",
    }
    assert all(request[1:3] == ("--paginate", "--slurp") for request in requests[1:])


def test_runner_absence_requires_every_applicable_scope(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    requested: list[str] = []

    async def request(*args: str) -> object:
        endpoint = args[-1]
        requested.append(endpoint)
        if endpoint == "repos/indexable-inc/ix":
            return {"owner": {"login": "indexable-inc", "type": "Organization"}}
        return _page()

    monkeypatch.setattr(github, "_request_json", request)
    result = asyncio.run(github.runner("indexable-inc/ix", "gone"))

    assert result.matches == []
    assert result.scopes_checked == ["repository", "organization"]
    assert len(requested) == 3


def test_runner_scope_failure_does_not_pose_as_absence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    async def request(*args: str) -> object:
        endpoint = args[-1]
        if endpoint == "repos/indexable-inc/ix":
            return {"owner": {"login": "indexable-inc", "type": "Organization"}}
        if endpoint.startswith("repos/indexable-inc/ix/actions/runners"):
            return _page()
        raise RuntimeError("organization runner endpoint denied")

    monkeypatch.setattr(github, "_request_json", request)

    with pytest.raises(RuntimeError, match="organization runner endpoint denied"):
        asyncio.run(github.runner("indexable-inc/ix", "unknown"))
