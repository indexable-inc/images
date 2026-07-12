"""Typed GitHub helpers for the ix-mcp kernel.

``runner`` resolves a repository's owner and checks every Actions runner scope
that can serve it. GitHub's repository endpoint omits organization-scoped
ephemeral runners, so an empty repository response alone is not evidence that a
runner disappeared.
"""

from __future__ import annotations

import asyncio
import re
from typing import Literal

from pydantic import BaseModel, TypeAdapter

__version__ = "0.1.0"

__all__ = ["RunnerLookup", "RunnerMatch", "runner"]

RunnerScope = Literal["repository", "organization"]

_REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")


class _Owner(BaseModel):
    login: str
    type: Literal["Organization", "User"]


class _Repository(BaseModel):
    owner: _Owner


class _Label(BaseModel):
    name: str


class _ApiRunner(BaseModel):
    id: int
    name: str
    status: str
    busy: bool
    labels: list[_Label]


class _RunnerPage(BaseModel):
    runners: list[_ApiRunner]


_RUNNER_PAGES = TypeAdapter(list[_RunnerPage])


class RunnerMatch(BaseModel):
    """One exact-name runner match and the API scope that reported it."""

    scope: RunnerScope
    id: int
    name: str
    status: str
    busy: bool
    labels: list[str]


class RunnerLookup(BaseModel):
    """The checked scopes and every exact-name runner match."""

    repository: str
    runner_name: str
    owner: str
    scopes_checked: list[RunnerScope]
    matches: list[RunnerMatch]


async def _request_json(*args: str) -> object:
    from sh import _exec

    output = await _exec(["gh", *args], check=True, color=False, timeout=30)
    return output.json()


async def _runners(endpoint: str) -> list[_ApiRunner]:
    raw = await _request_json(
        "api", "--paginate", "--slurp", f"{endpoint}?per_page=100"
    )
    pages = _RUNNER_PAGES.validate_python(raw)
    return [runner for page in pages for runner in page.runners]


async def runner(repository: str, name: str) -> RunnerLookup:
    """Find ``name`` across every Actions runner scope for ``repository``.

    ``repository`` must be ``owner/name``. Organization-owned repositories query
    both the repository and organization endpoints. User-owned repositories only
    have repository scope. An empty ``matches`` is conclusive for the listed
    ``scopes_checked`` because any API failure raises instead of returning a
    partial lookup.
    """
    normalized = repository.strip()
    if _REPOSITORY.fullmatch(normalized) is None:
        raise ValueError("repository must be owner/name")
    if not name:
        raise ValueError("runner name must not be empty")

    metadata = _Repository.model_validate(
        await _request_json("api", f"repos/{normalized}")
    )
    owner = metadata.owner
    scopes: list[tuple[RunnerScope, str]] = [
        ("repository", f"repos/{normalized}/actions/runners")
    ]
    if owner.type == "Organization":
        scopes.append(("organization", f"orgs/{owner.login}/actions/runners"))

    runners_by_scope = await asyncio.gather(
        *(_runners(endpoint) for _, endpoint in scopes)
    )
    matches = [
        RunnerMatch(
            scope=scope,
            id=candidate.id,
            name=candidate.name,
            status=candidate.status,
            busy=candidate.busy,
            labels=[label.name for label in candidate.labels],
        )
        for (scope, _), candidates in zip(scopes, runners_by_scope, strict=True)
        for candidate in candidates
        if candidate.name == name
    ]
    return RunnerLookup(
        repository=normalized,
        runner_name=name,
        owner=owner.login,
        scopes_checked=[scope for scope, _ in scopes],
        matches=matches,
    )
