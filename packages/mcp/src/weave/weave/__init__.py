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
    "QueryResult",
    "Weave",
    "assert_fact",
    "assert_facts",
    "chat",
    "delegate",
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

__version__ = "0.1.0"

_DEFAULT_URL = "http://127.0.0.1:7677"
_HASH_RE = re.compile(r"^(?:blake3:)?[0-9a-fA-F]{64}$")
_BATCH = 500


@dataclass(frozen=True)
class HashRef:
    """Explicit marker for a Weave hash value."""

    value: str


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

    async def delegate(
        self,
        prompt: str,
        *,
        name: str | None = None,
        model: str | None = None,
        system: str | None = None,
        topic: str | None = None,
        thread: str = "thread.main",
    ) -> str:
        """Append agent + task facts to the journal; return the task entity id.

        One ``assert_facts`` batch, in order: the agent entity ``agent-<name>``
        (``name`` defaults to ``worker-<6hex>``) with type/name plus
        model/system/topic when given, then the task entity ``task-<8hex>``
        with type/agent/prompt/name/thread/requested_by, then
        ``(task, state, "pending")`` strictly last. The pending fact is what
        dispatches, so a half-written task never runs. The weave app fulfills
        each pending task as a live interactive session attributed to the
        agent entity; ``requested_by`` is this kernel's own agent id
        (IX_WEAVE_AGENT, ``agent:main`` when unset).
        """

        name = name or f"worker-{secrets.token_hex(3)}"
        agent = f"agent-{name}"
        task = f"task-{secrets.token_hex(4)}"
        facts: list[tuple[str, str, Any]] = [
            (agent, "type", "agent"),
            (agent, "name", name),
        ]
        facts += [(agent, attr, value) for attr, value in (("model", model), ("system", system), ("topic", topic)) if value is not None]
        facts += [
            (task, "type", "task"),
            (task, "agent", agent),
            (task, "prompt", prompt),
            (task, "name", " ".join(prompt.split()[:5])),
            (task, "thread", thread),
            (task, "requested_by", os.environ.get("IX_WEAVE_AGENT") or "agent:main"),
            (task, "state", "pending"),
        ]
        await self.assert_facts(facts)
        return task

    async def result(self, task: str, *, timeout: float | None = None) -> str:
        """Block until ``task`` finishes; return its ``result`` fact text.

        Polls ``latest(task, state)`` every 0.5s until it reaches done,
        failed, or cancelled, then returns the task's ``result`` fact text
        ("" when the fulfiller wrote none). Raises TimeoutError once
        ``timeout`` seconds pass without a terminal state.
        """

        deadline = None if timeout is None else time.monotonic() + timeout
        while True:
            rows = (await self.query(f'?- latest("{task}", state, S).'))["rows"]
            if rows and rows[0][0] in ("done", "failed", "cancelled"):
                out = (await self.query(f'?- latest("{task}", result, R).'))["rows"]
                return out[0][0] if out else ""
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


async def delegate(
    prompt: str,
    *,
    name: str | None = None,
    model: str | None = None,
    system: str | None = None,
    topic: str | None = None,
    thread: str = "thread.main",
) -> str:
    return await _default.delegate(prompt, name=name, model=model, system=system, topic=topic, thread=thread)


async def result(task: str, *, timeout: float | None = None) -> str:
    return await _default.result(task, timeout=timeout)


async def status(entity: str, status: str) -> dict[str, Any]:
    return await _default.status(entity, status)
