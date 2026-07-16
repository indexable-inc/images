"""Async Weave 2 client for facts, queries, blobs, chat, and verbs.

Bundled like ``mesh``/``linear`` so every kernel can ``import weave`` and talk to
one shared Weave journal. All I/O is async and uses httpx; tests can replace
``_client`` with an ``httpx.MockTransport`` factory.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import secrets
import time
from collections.abc import AsyncIterator, Iterable, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    import httpx
    import polars

__all__ = [
    "HashRef",
    "OperationConflictError",
    "QueryResult",
    "TaskCancelledError",
    "TaskFailedError",
    "Weave",
    "append_operation",
    "assert_fact",
    "assert_facts",
    "chat",
    "get_blob",
    "hashref",
    "mint",
    "put_blob",
    "query",
    "result",
    "retract",
    "status",
    "watch",
]

__version__ = "0.2.0"

_DEFAULT_URL = "http://127.0.0.1:7677"
_HASH_RE = re.compile(r"^(?:blake3:)?[0-9a-fA-F]{64}$")
_BATCH = 500


@dataclass(frozen=True)
class HashRef:
    """Explicit marker for a Weave hash value."""

    value: str


class TaskFailedError(RuntimeError):
    """A Weave task published a terminal ``failed`` or ``lost`` state."""


class TaskCancelledError(RuntimeError):
    """A Weave task published a terminal ``cancelled`` or ``interrupted`` state."""


class OperationConflictError(RuntimeError):
    """An operation id was already committed with different writes."""

    def __init__(self, operation_id: str, detail: str) -> None:
        super().__init__(f"operation {operation_id!r} conflicts: {detail}")
        self.operation_id = operation_id


def hashref(h: str) -> HashRef:
    """Mark ``h`` as a hash ref; plain strings are never guessed as hashes."""

    if not _HASH_RE.fullmatch(h):
        raise ValueError("hashref must be blake3:<hex64> or <hex64>")
    return HashRef(h)


def mint(kind: str) -> str:
    """Mint a Weave entity id as ``kind:8hex``."""

    return f"{kind}:{secrets.token_hex(4)}"


def _ms() -> int:
    return int(time.time() * 1000)


def _url() -> str:
    return os.environ.get("WEAVE_URL") or _DEFAULT_URL


def _headers() -> dict[str, str]:
    token = os.environ.get("WEAVE_TOKEN")
    return {"X-Api-Key": token} if token else {}


def _client(**kwargs: object) -> httpx.AsyncClient:
    """Return a fresh ``httpx.AsyncClient`` configured for Weave."""

    import httpx

    return httpx.AsyncClient(base_url=_url(), headers=_headers(), **kwargs)  # type: ignore[arg-type]


def _wrap_value(value: object) -> dict[str, Any]:
    if isinstance(value, HashRef):
        return {"t": "hash", "v": value.value.removeprefix("blake3:")}
    if isinstance(value, bytes):
        raise TypeError("bytes are blobs; use put_blob() and hashref()")
    if isinstance(value, bool):
        return {"t": "bool", "v": value}
    if isinstance(value, int):
        return {"t": "int", "v": value}
    if isinstance(value, float):
        return {"t": "float", "v": value}
    if isinstance(value, str):
        return {"t": "str", "v": value}
    raise TypeError(f"unsupported Weave value {type(value).__name__}")


def _unwrap_value(value: object) -> object:
    if isinstance(value, dict) and "t" in value:
        t = value.get("t")
        v = value.get("v")
        if t == "hash" and isinstance(v, str) and not v.startswith("blake3:"):
            return f"blake3:{v}"
        return v
    return value


def _fact(entity: str, attr: str, value: object) -> dict[str, Any]:
    # WriteRequest wire shape: entity and value are tagged ApiValues, the
    # attr is a plain string (crates/protocol/src/api.rs).
    return {"fact": {"entity": _wrap_value(entity), "attr": attr, "value": _wrap_value(value)}}


class QueryResult(dict[str, Any]):
    """Weave query result with a Polars frame helper."""

    def frame(self) -> polars.DataFrame:
        """Return query rows as a Polars DataFrame with object-valued columns."""

        import polars as pl

        vars_ = list(self.get("vars", []))
        rows = self.get("rows", [])
        return pl.DataFrame([dict(zip(vars_, row, strict=False)) for row in rows])


class Weave:
    """Lazy async client for one Weave server."""

    async def assert_fact(self, entity: str, attr: str, value: object) -> dict[str, Any]:
        """Assert one fact and return ``{seq, id}``."""

        async with _client() as client:
            resp = await client.post("/api/facts", json=_fact(entity, attr, value))
            resp.raise_for_status()
            return resp.json()

    async def assert_facts(self, facts: Sequence[tuple[str, str, Any]]) -> list[dict[str, Any]]:
        """Assert facts in batches of 500 using array ``/api/facts`` writes."""

        out: list[dict[str, Any]] = []
        async with _client() as client:
            for i in range(0, len(facts), _BATCH):
                payload = [_fact(e, a, v) for e, a, v in facts[i : i + _BATCH]]
                resp = await client.post("/api/facts", json=payload)
                resp.raise_for_status()
                data = resp.json()
                out.extend(data if isinstance(data, list) else [data])
        return out

    async def append_operation(
        self,
        operation_id: str,
        facts: Sequence[tuple[str, str, object]],
    ) -> list[dict[str, object]]:
        """Atomically append an idempotent fact batch.

        The server commits the first write set for ``operation_id``. Repeating
        the same set returns its original acknowledgements; different writes
        raise :class:`OperationConflictError` without appending a fact.
        """

        payload = {
            "operation_id": operation_id,
            "writes": [_fact(entity, attr, value) for entity, attr, value in facts],
        }
        async with _client() as client:
            response = await client.post("/api/facts", json=payload)
            if response.status_code == 409:
                detail = response.text.strip() or "the operation id is already committed"
                raise OperationConflictError(operation_id, detail)
            response.raise_for_status()
            body: object = response.json()

        if not isinstance(body, dict):
            raise RuntimeError("operation response must be an object")
        returned_id = body.get("operation_id")
        acknowledgements = body.get("acks")
        if returned_id != operation_id or not isinstance(acknowledgements, list):
            raise RuntimeError("operation response has an invalid id or acknowledgements")

        parsed: list[dict[str, object]] = []
        for acknowledgement in acknowledgements:
            if not isinstance(acknowledgement, dict):
                raise RuntimeError("operation acknowledgement must be an object")
            seq = acknowledgement.get("seq")
            fact_id = acknowledgement.get("id")
            if isinstance(seq, bool) or not isinstance(seq, int) or not isinstance(fact_id, str):
                raise RuntimeError("operation acknowledgement has an invalid seq or id")
            parsed.append({"seq": seq, "id": fact_id})
        return parsed

    async def retract(self, fact_id: str) -> dict[str, Any]:
        """Retract a fact by id."""

        async with _client() as client:
            resp = await client.post("/api/facts", json={"retract": {"id": fact_id}})
            resp.raise_for_status()
            return resp.json()

    async def query(self, program: str, as_of: int | None = None) -> QueryResult:
        """Run a Datalog query and unwrap Weave values to Python scalars."""

        payload: dict[str, Any] = {"program": program}
        if as_of is not None:
            payload["as_of"] = as_of
        async with _client() as client:
            resp = await client.post("/api/query", json=payload)
            resp.raise_for_status()
            data = resp.json()
        rows = [[_unwrap_value(v) for v in row] for row in data.get("rows", [])]
        return QueryResult(vars=data.get("vars", []), rows=rows, as_of=data.get("as_of"))

    async def put_blob(self, body: bytes) -> str:
        """Store bytes in Weave CAS and return ``blake3:<hex>``."""

        async with _client() as client:
            resp = await client.post("/api/blob", content=body)
            resp.raise_for_status()
            h = resp.json()["hash"]
            return h if h.startswith("blake3:") else f"blake3:{h}"

    async def get_blob(self, h: str) -> bytes:
        """Fetch CAS bytes by ``blake3:<hex>`` or ``<hex>``."""

        async with _client() as client:
            resp = await client.get(f"/api/blob/{h.removeprefix('blake3:')}")
            resp.raise_for_status()
            return resp.content

    async def chat(
        self,
        text: str,
        to: str = "agent:main",
        author: str | None = None,
        from_: str | None = None,
        role: str | None = None,
        id: str | None = None,
    ) -> dict[str, Any]:
        """Write a message through ``/api/chat``."""

        extras = {
            key: val
            for key, val in (("author", author), ("from", from_), ("role", role), ("id", id))
            if val is not None
        }
        payload = {"text": text, "to": to, **extras}
        async with _client() as client:
            resp = await client.post("/api/chat", json=payload)
            resp.raise_for_status()
            return resp.json()

    async def watch(self, program: str, interval: float = 1.0) -> AsyncIterator[dict[str, list[Any]]]:
        """Yield added/removed/updated query-row batches on SSE head advances."""

        previous: dict[Any, list[Any]] = {}
        async with _client(timeout=None) as client, client.stream("GET", "/api/events") as resp:
            resp.raise_for_status()
            async for line in resp.aiter_lines():
                if not line.startswith("data:"):
                    continue
                await asyncio.sleep(0 if interval <= 0 else min(interval, 0.001))
                current_rows = (await self.query(program))["rows"]
                current = {row[0] if row else i: row for i, row in enumerate(current_rows)}
                added = [row for k, row in current.items() if k not in previous]
                removed = [row for k, row in previous.items() if k not in current]
                updated = [row for k, row in current.items() if k in previous and previous[k] != row]
                previous = current
                if added or removed or updated:
                    yield {"added": added, "removed": removed, "updated": updated}

    async def result(self, task: str, *, timeout: float | None = None) -> str:
        """Block until ``task`` finishes; return its ``result`` fact text.

        Polls ``latest(task, state)`` every 0.5s. ``done`` returns the durable
        ``result`` fact ("" when the worker wrote none); ``failed`` and
        ``lost`` (a fabric run whose runner died without a terminal fact,
        appended by ``fabric.reconcile``) raise :class:`TaskFailedError`;
        ``cancelled`` and ``interrupted`` (fabric's interrupt bridge) raise
        :class:`TaskCancelledError` -- each with the published terminal
        detail. Raises TimeoutError once ``timeout`` seconds pass without a
        terminal state. This journal read is completion authority; any
        channel wake is only a best-effort hint to inspect the durable
        result.
        """

        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            rows = (await self.query(f'?- latest("{task}", state, S).'))["rows"]
            if rows:
                state = rows[0][0]
                if state == "done":
                    out = (await self.query(f'?- latest("{task}", result, R).'))["rows"]
                    return out[0][0] if out else ""
                if state in ("failed", "lost"):
                    out = (await self.query(f'?- latest("{task}", error, R).'))["rows"]
                    detail = out[0][0] if out else ""
                    raise TaskFailedError(f"task {state}: {task}" + (f": {detail}" if detail else ""))
                if state in ("cancelled", "interrupted"):
                    out = (await self.query(f'?- latest("{task}", result, R).'))["rows"]
                    detail = out[0][0] if out else ""
                    raise TaskCancelledError(f"task {state}: {task}" + (f": {detail}" if detail else ""))
            if deadline is not None and time.monotonic() >= deadline:
                raise TimeoutError(f"task not finished after {timeout}s: {task}")
            await asyncio.sleep(0.5)

    async def status(self, entity: str, status: str) -> dict[str, Any]:
        """Assert a latest-wins status fact."""

        return await self.assert_fact(entity, "status", status)


_default = Weave()


async def assert_fact(entity: str, attr: str, value: object) -> dict[str, Any]:
    return await _default.assert_fact(entity, attr, value)


async def assert_facts(facts: Sequence[tuple[str, str, Any]]) -> list[dict[str, Any]]:
    return await _default.assert_facts(facts)


async def append_operation(
    operation_id: str,
    facts: Sequence[tuple[str, str, object]],
) -> list[dict[str, object]]:
    return await _default.append_operation(operation_id, facts)


async def retract(fact_id: str) -> dict[str, Any]:
    return await _default.retract(fact_id)


async def query(program: str, as_of: int | None = None) -> QueryResult:
    return await _default.query(program, as_of)


async def put_blob(body: bytes) -> str:
    return await _default.put_blob(body)


async def get_blob(h: str) -> bytes:
    return await _default.get_blob(h)


async def chat(
    text: str,
    to: str = "agent:main",
    author: str | None = None,
    from_: str | None = None,
    role: str | None = None,
    id: str | None = None,
) -> dict[str, Any]:
    return await _default.chat(text, to=to, author=author, from_=from_, role=role, id=id)


async def watch(program: str, interval: float = 1.0) -> AsyncIterator[dict[str, list[Any]]]:
    async for batch in _default.watch(program, interval):
        yield batch


async def result(task: str, *, timeout: float | None = None) -> str:
    return await _default.result(task, timeout=timeout)


async def status(entity: str, status: str) -> dict[str, Any]:
    return await _default.status(entity, status)
