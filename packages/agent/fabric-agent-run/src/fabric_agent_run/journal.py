"""The narrow Weave record-plane contract used by the agent runner."""

from __future__ import annotations

import asyncio
import os
import secrets
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Protocol

import httpx
from pydantic import BaseModel, ConfigDict

_DEFAULT_URL = "http://127.0.0.1:7677"


@dataclass(frozen=True)
class HashRef:
    """A CAS digest that must use Weave's hash wire type."""

    value: str


Fact = tuple[str, str, str | HashRef]


class Journal(Protocol):
    """Typed seam so lifecycle tests do not need an HTTP server."""

    async def put_blob(self, body: bytes) -> HashRef: ...

    async def assert_facts(self, facts: Sequence[Fact]) -> None: ...

    async def latest(self, entity: str, attr: str) -> str | None: ...


class _BlobResponse(BaseModel):
    model_config = ConfigDict(extra="forbid")

    hash: str


class _Value(BaseModel):
    model_config = ConfigDict(extra="ignore")

    t: str
    v: str | int | float | bool | None


class _QueryResponse(BaseModel):
    model_config = ConfigDict(extra="ignore")

    rows: list[list[_Value]]


def mint_task() -> str:
    """Mint the same task id shape as the bundled ``fabric`` module."""

    return f"task:{secrets.token_hex(4)}"


def _wire_value(value: str | HashRef) -> dict[str, str]:
    if isinstance(value, HashRef):
        return {"t": "hash", "v": value.value.removeprefix("blake3:")}
    return {"t": "str", "v": value}


def _wire_fact(entity: str, attr: str, value: str | HashRef) -> dict[str, object]:
    return {
        "fact": {
            "entity": {"t": "str", "v": entity},
            "attr": attr,
            "value": _wire_value(value),
        }
    }


class WeaveJournal:
    """Async HTTP client for the local Weave record plane."""

    def __init__(
        self,
        *,
        url: str | None = None,
        token: str | None = None,
        identity: str | None = None,
    ) -> None:
        self._url = url or os.environ.get("WEAVE_URL") or _DEFAULT_URL
        self._token = token if token is not None else os.environ.get("WEAVE_TOKEN")
        self._identity = (
            identity if identity is not None else os.environ.get("WEAVE_IDENTITY")
        )
        self._http = httpx.AsyncClient(base_url=self._url, headers=self._headers())

    def _headers(self) -> dict[str, str]:
        headers: dict[str, str] = {}
        if self._token:
            headers["X-Api-Key"] = self._token
        if self._identity:
            headers["tailscale-user-login"] = self._identity
        return headers

    async def put_blob(self, body: bytes) -> HashRef:
        response = await self._http.post("/api/blob", content=body)
        response.raise_for_status()
        parsed = _BlobResponse.model_validate_json(response.content)
        digest = parsed.hash
        return HashRef(digest if digest.startswith("blake3:") else f"blake3:{digest}")

    async def assert_facts(self, facts: Sequence[Fact]) -> None:
        if not facts:
            return
        payload = [_wire_fact(entity, attr, value) for entity, attr, value in facts]
        response = await self._http.post("/api/facts", json=payload)
        response.raise_for_status()

    async def latest(self, entity: str, attr: str) -> str | None:
        program = f'?- latest("{entity}", {attr}, V).'
        response = await self._http.post("/api/query", json={"program": program})
        response.raise_for_status()
        parsed = _QueryResponse.model_validate_json(response.content)
        if not parsed.rows or not parsed.rows[0]:
            return None
        value = parsed.rows[0][0].v
        return value if isinstance(value, str) else None

    async def close(self) -> None:
        """Release the one pooled HTTP client owned by this run."""

        await self._http.aclose()


async def wait_for_interrupt(
    journal: Journal,
    task: str,
    *,
    poll_seconds: float,
) -> None:
    """Return once the durable interrupt request is visible."""

    # A durable remote fact has no local event to await. Polling is the protocol.
    while await journal.latest(task, "interrupt") != "requested":  # noqa: ASYNC110
        await asyncio.sleep(poll_seconds)
