"""Async Weave 2 client for facts, queries, blobs, chat, and verbs.

Bundled like ``mesh``/``linear`` so every kernel can ``import weave`` and talk to
one shared Weave journal. All I/O is async and uses httpx; tests can replace
``_client`` with an ``httpx.MockTransport`` factory.

Two write surfaces:

- ``assert_fact``/``assert_facts``/``put_blob``: synchronous RPC, raises if
  the server is unreachable. For reads-own-writes callers.
- ``record``/``flush`` (+ :class:`Blob`): durable-local-first (index#3418).
  ``record`` appends to an fsync'd spool (:mod:`weave.spool`) and returns
  once the intent is durable on disk; a background flusher delivers to the
  server in append order whenever it is reachable. Spawn paths (fabric)
  must use this surface so a down server never blocks or loses intent.
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
import re
import secrets
import threading
import time
from collections.abc import AsyncIterator, Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Any

from . import spool as _spool

if TYPE_CHECKING:
    import httpx
    import polars

__all__ = [
    "Blob",
    "HashRef",
    "QueryResult",
    "TaskCancelledError",
    "TaskFailedError",
    "Weave",
    "assert_fact",
    "assert_facts",
    "chat",
    "get_blob",
    "hashref",
    "mint",
    "flush",
    "put_blob",
    "query",
    "record",
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


class TaskFailedError(RuntimeError):
    """A Weave task published a terminal ``failed`` or ``lost`` state."""


class TaskCancelledError(RuntimeError):
    """A Weave task published a terminal ``cancelled`` or ``interrupted`` state."""


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


class Blob:
    """A byte payload riding a fact value through :func:`record`.

    The bytes are spooled inline (durable before ``record`` returns) and land
    in CAS at drain time; the fact then carries the server's hash ref.
    """

    __slots__ = ("data",)

    def __init__(self, data: bytes) -> None:
        self.data = data


_spools: dict[Path, _spool.Spool] = {}
_spools_lock = threading.Lock()


def _spool_dir() -> Path:
    env = os.environ.get("WEAVE_SPOOL")
    if env:
        return Path(env)
    state = os.environ.get("XDG_STATE_HOME") or os.path.expanduser("~/.local/state")
    return Path(state) / "weave" / "spool"


def _send_spooled(items: list[Any]) -> None:
    # Runs on the spool flusher thread (no event loop there); httpx errors
    # propagate into the spool's retry/park classification.
    asyncio.run(_deliver(items))


async def _deliver(items: list[Any]) -> None:
    async with _client() as client:
        facts: list[dict[str, Any]] = []
        for item in items:
            blob_b64 = item.get("blob_b64")
            if blob_b64 is not None:
                resp = await client.post("/api/blob", content=base64.b64decode(blob_b64))
                resp.raise_for_status()
                h = str(resp.json()["hash"]).removeprefix("blake3:")
                facts.extend(
                    {"fact": {"entity": ref["entity"], "attr": ref["attr"], "value": {"t": "hash", "v": h}}}
                    for ref in item["refs"]
                )
            else:
                facts.append(item)
        for i in range(0, len(facts), _BATCH):
            resp = await client.post("/api/facts", json=facts[i : i + _BATCH])
            resp.raise_for_status()


def _rejected(exc: BaseException) -> bool:
    """An auth rejection is permanent for this process: the credential (or
    its absence) will not change under retry."""
    import httpx

    return isinstance(exc, httpx.HTTPStatusError) and exc.response.status_code in (401, 403)


def _default_spool() -> _spool.Spool:
    directory = _spool_dir()
    with _spools_lock:
        live = _spools.get(directory)
        if live is None or live.closed:
            live = _spool.Spool(directory, _send_spooled, url=_url(), permanent=_rejected)
            _spools[directory] = live
        return live


async def record(facts: Sequence[tuple[str, str, object]]) -> None:
    """Durably spool facts locally, then return; never a server round trip.

    The spool flusher delivers them to Weave in append order whenever it is
    reachable (at-least-once). A :class:`Blob` value rides the spool inline
    and becomes a CAS put plus a hash-valued fact at drain time. Use this on
    every path where recording is mandatory but the server must not gate the
    action (fabric spawn paths, index#3418); use ``assert_facts`` only when
    the caller needs the server ack (read-your-writes).
    """

    items: list[Any] = []
    for entity, attr, value in facts:
        if isinstance(value, Blob):
            items.append({
                "blob_b64": base64.b64encode(value.data).decode("ascii"),
                "refs": [{"entity": _wrap_value(entity), "attr": attr}],
            })
        else:
            items.append(_fact(entity, attr, value))
    sp = _default_spool()
    done = asyncio.get_running_loop().run_in_executor(None, sp.append_many, items)
    try:
        await asyncio.shield(done)
    except asyncio.CancelledError:
        # A cancelled caller (fabric's interrupt path records the terminal
        # state from `except CancelledError`) still needs THIS append durable
        # and ordered before any follow-up record: the executor thread cannot
        # be cancelled, so ride it out, then surface the cancellation.
        await asyncio.shield(done)
        raise


async def flush(timeout: float = 10.0) -> bool:
    """Block until every spool in this process drained (or ``timeout``)."""

    with _spools_lock:
        live = list(_spools.values())
    drained = True
    for sp in live:
        drained = await asyncio.to_thread(sp.flush, timeout) and drained
    return drained


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
