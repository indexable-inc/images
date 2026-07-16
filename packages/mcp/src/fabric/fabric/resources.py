from __future__ import annotations

import hashlib
import json
import os
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Protocol

import weave

if TYPE_CHECKING:
    import polars

__all__ = [
    "ResourceClaim",
    "ResourceClaimError",
    "ResourceKey",
    "ResourceLedger",
    "ResourceOwnedError",
    "ResourceTransferDenied",
    "ResourceTransitionError",
    "claim",
    "current",
    "current_owner",
    "frame",
    "ledger",
    "release",
    "transfer",
]

_TERMINAL_OWNER_STATES = frozenset({"done", "failed", "interrupted", "lost"})
_ACTIVE = "active"
_RELEASED = "released"


class _Journal(Protocol):
    async def append_operation(
        self,
        operation_id: str,
        facts: Sequence[tuple[str, str, object]],
    ) -> list[dict[str, object]]: ...

    async def query(
        self, program: str, as_of: int | None = None
    ) -> weave.QueryResult: ...


@dataclass(frozen=True, slots=True)
class ResourceKey:
    """One mutation boundary in one repository."""

    repository: str
    pr: int | None = None
    worktree: str | None = None

    def __post_init__(self) -> None:
        repository = self.repository.strip().lower().removesuffix(".git")
        parts = repository.split("/")
        if len(parts) != 2 or any(
            not part or any(char.isspace() for char in part) for part in parts
        ):
            raise ValueError("repository must be a canonical owner/name slug")
        if (self.pr is None) == (self.worktree is None):
            raise ValueError("resource key needs exactly one of pr or worktree")
        if self.pr is not None and self.pr <= 0:
            raise ValueError("pull request number must be positive")
        object.__setattr__(self, "repository", repository)
        if self.worktree is not None:
            path = Path(self.worktree).expanduser()
            if not path.is_absolute():
                raise ValueError("worktree path must be absolute")
            object.__setattr__(self, "worktree", str(path.resolve(strict=False)))

    @classmethod
    def for_pull_request(cls, repository: str, number: int) -> ResourceKey:
        return cls(repository=repository, pr=number)

    @classmethod
    def for_worktree(cls, repository: str, path: str | Path) -> ResourceKey:
        return cls(repository=repository, worktree=str(path))

    @property
    def selector_kind(self) -> str:
        return "pr" if self.pr is not None else "worktree"

    @property
    def selector(self) -> int | str:
        if self.pr is not None:
            return self.pr
        if self.worktree is None:
            raise AssertionError("validated resource key has no selector")
        return self.worktree

    @property
    def entity(self) -> str:
        return f"mutation-resource:{_digest(self.repository, self.selector_kind, self.selector)}"

    @property
    def label(self) -> str:
        if self.pr is not None:
            return f"{self.repository}#{self.pr}"
        return f"{self.repository}:{self.worktree}"


@dataclass(frozen=True, slots=True)
class ResourceClaim:
    """The current owner generation for a resource."""

    key: ResourceKey
    id: str
    owner: str | None
    state: str
    owner_state: str | None = None
    requested_by: str | None = None

    @property
    def available(self) -> bool:
        return self.state == _RELEASED or self.owner_state in _TERMINAL_OWNER_STATES


class ResourceClaimError(RuntimeError):
    """Base class for loud resource admission failures."""


class ResourceOwnedError(ResourceClaimError):
    """A live owner already holds the mutation boundary."""

    def __init__(self, key: ResourceKey, claim: ResourceClaim) -> None:
        owner = claim.owner or "unknown"
        super().__init__(
            f"mutation resource {key.label} is already owned by {owner}; "
            "the current owner must transfer or release it"
        )
        self.key = key
        self.claim = claim


class ResourceTransferDenied(ResourceClaimError):
    """A caller other than the current owner attempted a handoff."""

    def __init__(self, claim: ResourceClaim, actor: str) -> None:
        super().__init__(
            f"resource claim {claim.id} belongs to {claim.owner or 'nobody'}, not {actor}"
        )
        self.claim = claim
        self.actor = actor


class ResourceTransitionError(ResourceClaimError):
    """The ownership head changed while a transition was being committed."""

    def __init__(
        self,
        key: ResourceKey,
        expected: ResourceClaim | None,
        observed: ResourceClaim | None,
    ) -> None:
        expected_id = expected.id if expected is not None else "unclaimed"
        observed_id = observed.id if observed is not None else "unclaimed"
        super().__init__(
            f"resource {key.label} changed from {expected_id} to {observed_id}; "
            "read the ledger before mutating"
        )
        self.key = key
        self.expected = expected
        self.observed = observed


def current_owner() -> str:
    """The journal identity for this agent subtree."""

    return os.environ.get("IX_WEAVE_AGENT") or "agent:main"


def _digest(*parts: object) -> str:
    body = json.dumps(parts, separators=(",", ":"), sort_keys=True).encode()
    return hashlib.blake2s(body, digest_size=16).hexdigest()


def _claim_id(key: ResourceKey, predecessor: str, owner: str, state: str) -> str:
    return f"resource-claim:{_digest(key.entity, predecessor, owner, state)}"


def _operation_id(predecessor: str) -> str:
    return f"resource_claim_{_digest(predecessor)}"


def _string(value: str) -> str:
    return json.dumps(value)


def _current_query(resource: str) -> str:
    entity = _string(resource)
    return (
        f"?- latest({entity}, current_claim, C), "
        "latest(C, claimed_by, O), latest(C, state, S)."
    )


def _latest_query(entity: str, attr: str) -> str:
    if not attr.isidentifier():
        raise ValueError(f"invalid weave attribute {attr!r}")
    return f"?- latest({_string(entity)}, {attr}, V)."


_LEDGER_QUERY = (
    '?- type(R, "mutation_resource"), latest(R, repository, Repo), '
    "latest(R, selector_kind, K), latest(R, selector, V), "
    "latest(R, current_claim, C), latest(C, claimed_by, O), latest(C, state, S)."
)


def _rows(result: weave.QueryResult) -> list[list[object]]:
    raw: object = result.get("rows")
    if not isinstance(raw, list):
        raise RuntimeError("weave query response has no rows list")
    rows: list[list[object]] = []
    for row in raw:
        if not isinstance(row, list):
            raise RuntimeError("weave query row must be a list")
        rows.append(list(row))
    return rows


class ResourceLedger:
    """Atomic mutation ownership backed by the shared weave journal.

    Weave scopes operation ids to one authenticated principal. Parent and child
    sessions on an MCP service share that principal. Exclusion across separate
    principals requires a server-owned conditional write primitive.
    """

    def __init__(self, journal: _Journal | None = None) -> None:
        self._journal = journal or weave.Weave()

    async def _owner_attr(self, owner: str | None, attr: str) -> str | None:
        if not owner:
            return None
        rows = _rows(await self._journal.query(_latest_query(owner, attr)))
        if not rows:
            return None
        if len(rows) != 1 or len(rows[0]) != 1 or not isinstance(rows[0][0], str):
            raise RuntimeError(f"owner {attr} query returned an invalid row for {owner}")
        return rows[0][0]

    async def current(self, key: ResourceKey) -> ResourceClaim | None:
        rows = _rows(await self._journal.query(_current_query(key.entity)))
        if not rows:
            return None
        if len(rows) != 1 or len(rows[0]) != 3:
            raise RuntimeError(
                f"resource query returned an invalid head for {key.label}"
            )
        claim_id, owner, state = rows[0]
        if (
            not isinstance(claim_id, str)
            or not isinstance(owner, str)
            or not isinstance(state, str)
        ):
            raise RuntimeError(
                f"resource query returned invalid values for {key.label}"
            )
        return ResourceClaim(
            key=key,
            id=claim_id,
            owner=owner or None,
            state=state,
            owner_state=await self._owner_attr(owner or None, "state"),
            requested_by=await self._owner_attr(owner or None, "requested_by"),
        )

    def _resource_facts(self, key: ResourceKey) -> list[tuple[str, str, object]]:
        return [
            (key.entity, "type", "mutation_resource"),
            (key.entity, "repository", key.repository),
            (key.entity, "selector_kind", key.selector_kind),
            (key.entity, "selector", key.selector),
        ]

    def _claim_facts(
        self,
        claim: ResourceClaim,
        predecessor: ResourceClaim | None,
    ) -> list[tuple[str, str, object]]:
        facts: list[tuple[str, str, object]] = [
            (claim.id, "type", "resource_claim"),
            (claim.id, "resource", claim.key.entity),
            (claim.id, "claimed_by", claim.owner or ""),
            (claim.id, "state", claim.state),
        ]
        if predecessor is not None:
            facts.append((claim.id, "previous_claim", predecessor.id))
        return facts

    async def _append(
        self,
        key: ResourceKey,
        predecessor: ResourceClaim | None,
        claim: ResourceClaim,
        *,
        acknowledged_by: str | None = None,
    ) -> ResourceClaim:
        operation_predecessor = (
            predecessor.id if predecessor is not None else key.entity
        )
        facts = self._resource_facts(key) if predecessor is None else []
        if predecessor is not None:
            facts.extend(
                [
                    (predecessor.id, "state", "superseded"),
                    (predecessor.id, "next_claim", claim.id),
                ]
            )
            if acknowledged_by is not None:
                facts.append((predecessor.id, "acknowledged_by", acknowledged_by))
        facts.extend(self._claim_facts(claim, predecessor))
        facts.append((key.entity, "current_claim", claim.id))
        await self._journal.append_operation(
            _operation_id(operation_predecessor), facts
        )
        return claim

    async def claim(
        self, key: ResourceKey, *, owner: str | None = None
    ) -> ResourceClaim:
        holder = owner or current_owner()
        predecessor = await self.current(key)
        if predecessor is not None and not predecessor.available:
            if predecessor.owner == holder:
                return predecessor
            raise ResourceOwnedError(key, predecessor)

        predecessor_id = predecessor.id if predecessor is not None else "root"
        next_claim = ResourceClaim(
            key=key,
            id=_claim_id(key, predecessor_id, holder, _ACTIVE),
            owner=holder,
            state=_ACTIVE,
        )
        try:
            return await self._append(key, predecessor, next_claim)
        except weave.OperationConflictError as source:
            observed = await self.current(key)
            if (
                observed is not None
                and observed.owner == holder
                and not observed.available
            ):
                return observed
            if observed is not None and not observed.available:
                raise ResourceOwnedError(key, observed) from source
            raise ResourceTransitionError(key, predecessor, observed) from source

    async def transfer(
        self,
        claim: ResourceClaim,
        to_owner: str,
        *,
        actor: str | None = None,
    ) -> ResourceClaim:
        acknowledged_by = actor or current_owner()
        if claim.owner != acknowledged_by:
            raise ResourceTransferDenied(claim, acknowledged_by)
        current = await self.current(claim.key)
        if current is None or current.id != claim.id or current.owner != claim.owner:
            raise ResourceTransitionError(claim.key, claim, current)
        if current.available:
            raise ResourceTransitionError(claim.key, claim, current)
        if to_owner == current.owner:
            return current

        next_claim = ResourceClaim(
            key=claim.key,
            id=_claim_id(claim.key, claim.id, to_owner, _ACTIVE),
            owner=to_owner,
            state=_ACTIVE,
        )
        try:
            return await self._append(
                claim.key,
                current,
                next_claim,
                acknowledged_by=acknowledged_by,
            )
        except weave.OperationConflictError as source:
            observed = await self.current(claim.key)
            if (
                observed is not None
                and observed.owner == to_owner
                and not observed.available
            ):
                return observed
            raise ResourceTransitionError(claim.key, claim, observed) from source

    async def release(
        self,
        claim: ResourceClaim,
        *,
        actor: str | None = None,
    ) -> ResourceClaim:
        acknowledged_by = actor or current_owner()
        if claim.owner != acknowledged_by:
            raise ResourceTransferDenied(claim, acknowledged_by)
        current = await self.current(claim.key)
        if current is None or current.id != claim.id or current.owner != claim.owner:
            if current is not None and current.state == _RELEASED:
                return current
            raise ResourceTransitionError(claim.key, claim, current)

        released = ResourceClaim(
            key=claim.key,
            id=_claim_id(claim.key, claim.id, "", _RELEASED),
            owner=None,
            state=_RELEASED,
        )
        try:
            return await self._append(
                claim.key,
                current,
                released,
                acknowledged_by=acknowledged_by,
            )
        except weave.OperationConflictError as source:
            observed = await self.current(claim.key)
            if observed is not None and observed.state == _RELEASED:
                return observed
            raise ResourceTransitionError(claim.key, claim, observed) from source

    async def ledger(self) -> list[ResourceClaim]:
        rows = _rows(await self._journal.query(_LEDGER_QUERY))
        claims: list[ResourceClaim] = []
        for row in rows:
            if len(row) != 7:
                raise RuntimeError("resource ledger query returned an invalid row")
            resource, repository, kind, selector, claim_id, owner, state = row
            if (
                not isinstance(resource, str)
                or not isinstance(repository, str)
                or not isinstance(kind, str)
                or not isinstance(claim_id, str)
                or not isinstance(owner, str)
                or not isinstance(state, str)
            ):
                raise RuntimeError("resource ledger query returned invalid values")
            if (
                kind == "pr"
                and isinstance(selector, int)
                and not isinstance(selector, bool)
            ):
                key = ResourceKey.for_pull_request(repository, selector)
            elif kind == "worktree" and isinstance(selector, str):
                key = ResourceKey.for_worktree(repository, selector)
            else:
                raise RuntimeError(
                    f"resource ledger has invalid selector kind {kind!r}"
                )
            if key.entity != resource:
                raise RuntimeError(
                    f"resource ledger key does not match entity {resource}"
                )
            claims.append(
                ResourceClaim(
                    key=key,
                    id=claim_id,
                    owner=owner or None,
                    state=state,
                    owner_state=await self._owner_attr(owner or None, "state"),
                    requested_by=await self._owner_attr(owner or None, "requested_by"),
                )
            )
        return sorted(claims, key=lambda item: item.key.label)

    async def frame(self) -> polars.DataFrame:
        import polars as pl

        rows: list[dict[str, str | int | None]] = [
            {
                "repository": claim.key.repository,
                "kind": claim.key.selector_kind,
                "selector": claim.key.selector,
                "owner": claim.owner,
                "requested_by": claim.requested_by,
                "owner_state": claim.owner_state,
                "state": claim.state,
                "claim": claim.id,
            }
            for claim in await self.ledger()
        ]
        return pl.DataFrame(rows)


_default = ResourceLedger()


async def current(key: ResourceKey) -> ResourceClaim | None:
    return await _default.current(key)


async def claim(key: ResourceKey, *, owner: str | None = None) -> ResourceClaim:
    return await _default.claim(key, owner=owner)


async def transfer(
    claim: ResourceClaim,
    to_owner: str,
    *,
    actor: str | None = None,
) -> ResourceClaim:
    return await _default.transfer(claim, to_owner, actor=actor)


async def release(
    claim: ResourceClaim,
    *,
    actor: str | None = None,
) -> ResourceClaim:
    return await _default.release(claim, actor=actor)


async def ledger() -> list[ResourceClaim]:
    return await _default.ledger()


async def frame() -> polars.DataFrame:
    return await _default.frame()
