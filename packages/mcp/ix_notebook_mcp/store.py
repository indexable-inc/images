"""Weave-backed execution store facade.

Public functions keep the historical ``fn(conn, ...)`` surface. ``conn`` is now a
:class:`WeaveStore` handle. Durable writes become Weave facts; tier-3 mailbox
state is accessed through the serve data API.
"""

from __future__ import annotations

import asyncio
import contextlib
import functools
import hashlib
import json
import os
import sys
import threading
import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import TYPE_CHECKING, Any, TypeVar
from urllib.parse import quote
from urllib.request import Request, urlopen

if TYPE_CHECKING:
    from collections.abc import Callable
    from typing import Concatenate, ParamSpec

    _P = ParamSpec("_P")
_T = TypeVar("_T")

_DEFAULT_WEAVE_URL = "http://127.0.0.1:7677"
_DEFAULT_DATA_API = "http://127.0.0.1:8765"
_BATCH = 500
_QUEUE_MAX = 10_000
_WARNED_OFF = False

def _http_json(method: str, url: str, *, body: object = None, content: bytes | None = None) -> object:
    headers: dict[str, str] = {}
    data: bytes | None = content
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = Request(url, data=data, headers=headers, method=method)  # noqa: S310 - configured local/Weave endpoint
    with urlopen(req, timeout=10.0) as resp:  # noqa: S310
        raw = resp.read()
    if not raw:
        return None
    return json.loads(raw.decode("utf-8"))


def _http_bytes(method: str, url: str, *, content: bytes | None = None) -> bytes:
    req = Request(url, data=content, method=method)  # noqa: S310 - configured local/Weave endpoint
    with urlopen(req, timeout=10.0) as resp:  # noqa: S310
        return resp.read()


def _now() -> float:
    return time.time()


def _ms(seconds: float | None = None) -> int:
    return round((seconds if seconds is not None else _now()) * 1000)


def _sec(ms: object) -> float | None:
    if ms is None or ms == "":
        return None
    return int(ms) / 1000.0


def _stable8(path: str | Path) -> str:
    # blake3 is not a workspace dependency in ix-mcp; sha256 gives a stable
    # path-derived id without adding a persistence-only dependency.
    return hashlib.sha256(str(path).encode("utf-8")).hexdigest()[:8]


def _entity(prefix: str, id: str) -> str:
    return id if id.startswith(f"{prefix}:") else f"{prefix}:{id}"


class _HashRef(str):
    """Marks a value as a CAS blob reference so it rides as a typed hash."""


def _api_value(value: object) -> dict:
    # bool must be checked before int (bool is an int subclass).
    if isinstance(value, _HashRef):
        return {"t": "hash", "v": str(value)}
    if isinstance(value, bool):
        return {"t": "bool", "v": value}
    if isinstance(value, int):
        return {"t": "int", "v": value}
    if isinstance(value, float):
        return {"t": "float", "v": value}
    return {"t": "str", "v": str(value)}


def _unwrap(cell: object) -> object:
    if isinstance(cell, dict) and "t" in cell and "v" in cell:
        return cell["v"]
    return cell


def _json_blob(value: object) -> bytes:
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode("utf-8")


class WeaveStore:
    def __init__(self, path: str | Path) -> None:
        if path is None:
            raise ValueError("store path is required (got None)")
        self.path = str(path)
        suffix = _stable8(path)
        self.weave_url = os.environ.get("WEAVE_URL", _DEFAULT_WEAVE_URL).rstrip("/")
        self.disabled = self.weave_url.lower() == "off"
        self.agent = os.environ.get("IX_WEAVE_AGENT") or f"agent:{suffix}"
        self.kernel = f"kernel:{suffix}"
        self.mailbox_base = os.environ.get("IX_MCP_DATA_API_URL", _DEFAULT_DATA_API).rstrip("/")
        self._queue: deque[dict] = deque(maxlen=_QUEUE_MAX)
        self._cv = threading.Condition()
        self._closed = False
        self._inflight = False
        self._thread: threading.Thread | None = None
        self._blob_cache: dict[str, str] = {}
        self._last_values: dict[tuple[str, str], Any] = {}
        global _WARNED_OFF
        if self.disabled:
            if not _WARNED_OFF:
                print("ix-mcp store: WEAVE_URL=off, persistence writes are disabled", file=sys.stderr)
                _WARNED_OFF = True
        else:
            self._thread = threading.Thread(target=self._writer, name="ix-weave-writer", daemon=True)
            self._thread.start()
            now = _ms()
            self._enqueue_facts([
                (self.agent, "type", "agent"),
                (self.agent, "label", self.agent.removeprefix("agent:")),
                (self.agent, "client", "ix-mcp"),
                (self.agent, "connected_ms", now),
                (self.agent, "last_active_ms", now),
                (self.kernel, "type", "kernel"),
                (self.kernel, "transport", "mcp"),
                (self.kernel, "pid", os.getpid()),
                (self.agent, "on_kernel", self.kernel),
            ])

    def close(self) -> None:
        # Best-effort drain so queued facts do not die with the process.
        self.flush(timeout=5.0)
        self._closed = True
        with self._cv:
            self._cv.notify_all()

    def flush(self, timeout: float = 10.0) -> bool:
        """Block until the write-behind queue is fully drained (or timeout)."""
        if self.disabled:
            return True
        deadline = time.monotonic() + timeout
        with self._cv:
            while (self._queue or self._inflight) and time.monotonic() < deadline:
                self._cv.wait(timeout=0.1)
            return not self._queue and not self._inflight

    def _enqueue(self, item: dict) -> None:
        if self.disabled:
            return
        with self._cv:
            if len(self._queue) == self._queue.maxlen:
                print("ix-mcp store: write-behind cache full, dropping oldest fact", file=sys.stderr)
            self._queue.append(item)
            self._cv.notify()

    def _enqueue_facts(self, facts: list[tuple[str, str, Any]]) -> None:
        if not facts:
            return
        facts = [*facts, (self.agent, "last_active_ms", _ms())]
        for entity, attr, value in facts:
            key = (entity, attr)
            if self._last_values.get(key) == value:
                continue
            self._last_values[key] = value
            self._enqueue({"fact": {"entity": _api_value(entity), "attr": attr, "value": _api_value(value)}})

    def _enqueue_blob_fact(self, entity: str, attr: str, data: bytes) -> None:
        """Queue one (entity, attr, <hash of data>) fact whose CAS put is
        deferred to the writer thread (see _resolve_blob_item). The enqueue is
        a plain list append, so bulk payloads (pty chunks) never block the
        caller on a synchronous /api/blob round trip; FIFO keeps journal seq =
        enqueue order. Deliberately NOT routed through _enqueue_facts: its
        last-value dedupe would eat a repeated identical chunk, and replay
        needs every flush."""
        if self.disabled:
            return
        self._enqueue({"blob_fact": {"entity": entity, "attr": attr, "data": data}})

    def put_blob(self, data: bytes) -> _HashRef:
        if self.disabled:
            return _HashRef(hashlib.sha256(data).hexdigest())
        digest = hashlib.sha256(data).hexdigest()
        cached = self._blob_cache.get(digest)
        if cached:
            return _HashRef(cached)
        h = str(_http_json("POST", f"{self.weave_url}/api/blob", content=data)["hash"])
        self._blob_cache[digest] = h
        return _HashRef(h)

    def get_blob(self, hash_: str) -> bytes:
        if self.disabled or not hash_:
            return b""
        return _http_bytes("GET", f"{self.weave_url}/api/blob/{quote(hash_, safe='')}")

    def _resolve_blob_item(self, item: dict) -> dict:
        """blob_fact items defer their CAS put to the writer thread: PUT the
        bytes (the server computes the blake3 hash) and rewrite the item into
        a plain fact carrying the returned hash ref. A put failure raises into
        _writer's retry path, which requeues the ORIGINAL items; put_blob's
        digest cache makes the retry cheap."""
        blob = item.get("blob_fact")
        if blob is None:
            return item
        h = self.put_blob(blob["data"])
        return {"fact": {"entity": _api_value(blob["entity"]), "attr": blob["attr"], "value": _api_value(h)}}

    def _writer(self) -> None:
        backoff = 0.25
        while True:
            with self._cv:
                while not self._queue and not self._closed:
                    self._cv.wait(timeout=1.0)
                if not self._queue and self._closed:
                    return
                batch = [self._queue.popleft() for _ in range(min(_BATCH, len(self._queue)))]
                self._inflight = True
            try:
                body = [self._resolve_blob_item(item) for item in batch]
                _http_json("POST", f"{self.weave_url}/api/facts", body=body if len(body) != 1 else body[0])
                backoff = 0.25
                with self._cv:
                    self._inflight = False
                    self._cv.notify_all()
            except Exception as exc:
                print(f"ix-mcp store: weave write failed, retrying: {exc}", file=sys.stderr)
                with self._cv:
                    for item in reversed(batch):
                        self._queue.appendleft(item)
                    self._inflight = False
                time.sleep(backoff)
                backoff = min(backoff * 2, 5.0)

    def query(self, program: str, *, as_of: int | None = None) -> dict:
        # Sync reads are only used from runtime worker paths or AsyncConn executor
        # threads; they intentionally use blocking stdlib HTTP off the event loop.
        if self.disabled:
            return {"vars": [], "rows": [], "as_of": None}
        # Read-your-writes: the sqlite store was synchronous, and callers
        # (session restore, replay anchoring) depend on seeing their own
        # writes. Drain the write-behind queue before answering.
        self.flush(timeout=10.0)
        payload: dict[str, Any] = {"program": program}
        if as_of is not None:
            payload["as_of"] = as_of
        return _http_json("POST", f"{self.weave_url}/api/query", body=payload)

    def mailbox(self, method: str, path: str, *, json_body: object = None) -> object:
        try:
            return _http_json(method, f"{self.mailbox_base}{path}", body=json_body)
        except Exception:
            from .mailbox import get_mailbox
            box = get_mailbox()
            if method == "POST" and path == "/api/input":
                channel = str(json_body["channel"])
                if not box.channel_open(channel):
                    raise ValueError("no such open channel") from None
                box.add_input(channel=channel, payload=json.dumps(json_body["payload"]))
                return {"ok": True}
            if method == "POST" and path == "/api/mailbox/outbox":
                box.add_outbox(content=str(json_body.get("content", "")), meta=str(json_body.get("meta", "{}")), session=str(json_body.get("session", "")))
                return {"ok": True}
            if method == "POST" and path == "/api/mailbox/inputs/delete":
                box.delete_inputs([int(s) for s in (json_body or {}).get("seqs", [])])
                return {"ok": True}
            if method == "GET" and path.startswith("/api/mailbox/inputs"):
                rows = box.pending_inputs()
                if "consume=1" in path:
                    box.delete_inputs([row["seq"] for row in rows])
                return rows
            if method == "POST" and path == "/api/mailbox/events":
                box.add_event(resource=str(json_body["resource"]), kind=str(json_body["kind"]), body=str(json_body["body"]))
                return {"ok": True}
            if method == "POST" and path == "/api/mailbox/channels":
                if json_body.get("op") == "open":
                    box.open_channel(id=str(json_body["id"]), title=str(json_body.get("title", "")))
                else:
                    box.close_channel(id=str(json_body["id"]))
                return {"ok": True}
            if method == "GET" and path.startswith("/api/mailbox/channels/"):
                return {"open": box.channel_open(path.rsplit("/", 1)[-1])}
            if method == "POST" and path == "/api/mailbox/reset":
                box.reset()
                return {"ok": True}
            raise


def connect(path: str | Path) -> WeaveStore:
    return WeaveStore(path)


class AsyncConn:
    def __init__(self, path: str | Path) -> None:
        if path is None:
            raise ValueError("store path is required (got None)")
        self._path = path
        self._pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="ix-store")
        self._conn: WeaveStore | None = None

    def _bound(self) -> WeaveStore:
        if self._conn is None:
            self._conn = connect(self._path)
        return self._conn

    def _invoke(self, fn: Callable[Concatenate[WeaveStore, _P], _T], args: tuple, kwargs: dict) -> _T:
        return fn(self._bound(), *args, **kwargs)

    async def run(self, fn: Callable[Concatenate[WeaveStore, _P], _T], /, *args: _P.args, **kwargs: _P.kwargs) -> _T:
        loop = asyncio.get_running_loop()
        return await loop.run_in_executor(self._pool, functools.partial(self._invoke, fn, args, kwargs))

    async def close(self) -> None:
        def _close() -> None:
            if self._conn is not None:
                self._conn.close()
                self._conn = None
        await asyncio.get_running_loop().run_in_executor(self._pool, _close)
        self._pool.shutdown(wait=False)


def _blob(conn: WeaveStore, value: object) -> _HashRef:
    return conn.put_blob(_json_blob(value))


def _blob_text(conn: WeaveStore, text: str) -> _HashRef:
    return conn.put_blob(text.encode("utf-8"))


def _load_json_blob(conn: WeaveStore, hash_: str, default: object) -> object:
    if not hash_:
        return default
    try:
        return json.loads(conn.get_blob(hash_).decode("utf-8"))
    except Exception:
        return default


def _load_text_blob(conn: WeaveStore, hash_: str) -> str:
    if not hash_:
        return ""
    try:
        return conn.get_blob(hash_).decode("utf-8")
    except Exception:
        return ""


def _rows(result: dict) -> list[dict]:
    vars_ = result.get("vars") or []
    out = []
    for row in result.get("rows") or []:
        if isinstance(row, dict):
            out.append({k: _unwrap(v) for k, v in row.items()})
        else:
            out.append({k: _unwrap(v) for k, v in zip(vars_, row, strict=False)})
    return out


def _pivot(conn: WeaveStore, program: str) -> dict[str, dict[str, Any]]:
    """Run a `?- ... latest(E, A, V).` program and fold rows into
    {entity: {attr: value}} - one round trip fetches every attribute,
    optional attrs simply absent (a wide datalog join would drop rows)."""
    entities: dict[str, dict[str, Any]] = {}
    for row in _rows(conn.query(program)):
        ent = str(row.get("E") or "")
        if not ent:
            continue
        entities.setdefault(ent, {})[str(row.get("A"))] = row.get("V")
    return entities


def _children(conn: WeaveStore, types: tuple[str, ...]) -> dict[str, dict[str, Any]]:
    attrs = _pivot(conn, f"?- child_of(E, {conn.agent}), latest(E, A, V).")
    return {ent: a for ent, a in attrs.items() if a.get("type") in types}


def _attrs_of(conn: WeaveStore, ent: str) -> dict[str, Any]:
    """All latest attrs of one entity (the entity is a query constant, so
    rows carry only A and V)."""
    attrs: dict[str, Any] = {}
    for row in _rows(conn.query(f"?- latest({ent}, A, V).")):
        attrs[str(row.get("A"))] = row.get("V")
    return attrs


def _exec_from_attrs(conn: WeaveStore, ent: str, a: dict[str, Any]) -> dict:
    return {
        "id": str(ent).split(":", 1)[-1],
        "name": a.get("desc") or "",
        "code": _load_text_blob(conn, a.get("code") or ""),
        "status": a.get("status") or "running",
        "started_at": _sec(a.get("started_ms")),
        "ended_at": _sec(a.get("ended_ms")),
        "budget": float(a.get("budget") or 15.0),
        "output": a.get("last_output") or "",
        "result": _load_text_blob(conn, a["result"]) if a.get("result") else None,
        "error": a.get("error"),
        # The live line is meaningful only while running (derived, not
        # cleared by a write: truth is derived, never stored - I2).
        "line": a.get("line") if a.get("status") == "running" else None,
        "error_line": a.get("error_line"),
        "outputs": _load_json_blob(conn, a.get("outputs") or "", []),
        "bindings": _load_json_blob(conn, a.get("bindings") or "", {}),
        "kind": a.get("kind") or "cell",
        "topic": a.get("topic") or "",
    }


def start(conn: WeaveStore, *, id: str, name: str, code: str, started_at: float, budget: float = 15.0, kind: str = "cell", topic: str = "") -> None:
    ent = _entity("run", id) if kind != "spawn" else _entity("proc", id)
    conn._enqueue_facts([
        (ent, "type", "process" if kind == "spawn" else "run"),
        (ent, "child_of", conn.agent),
        (ent, "on_kernel", conn.kernel),
        (ent, "verb", "python_exec"),
        (ent, "desc", name),
        (ent, "topic", topic),
        (ent, "code", _blob_text(conn, code)),
        (ent, "status", "running"),
        (ent, "started_ms", _ms(started_at)),
        (ent, "budget", budget),
        (ent, "kind", kind),
    ])


def rename(conn: WeaveStore, *, id: str, name: str) -> None:
    conn._enqueue_facts([(_entity("run", id), "desc", name), (_entity("proc", id), "desc", name)])


def update_output(conn: WeaveStore, id: str, output: str, outputs: list | None = None, *, line: int | None = None) -> None:
    preview = output[-200:] if output else ""
    facts: list[tuple[str, str, Any]] = [(_entity("run", id), "last_output", preview)]
    if line is not None:
        facts.append((_entity("run", id), "line", line))
    if outputs is not None:
        facts.append((_entity("run", id), "outputs", _blob(conn, outputs)))
    conn._enqueue_facts(facts)


def stream_open(conn: WeaveStore, *, id: str, kind: str = "cell") -> str:
    """Mint the pty_stream entity for one run/process (weave2 n-pty; schema.dl
    "PTY/CAS stream shapes"): (S, type, pty_stream) + (S, child_of, parent),
    parent picked exactly like start() so the stream tombstones with its run.
    Chunks carry no seq attr - the journal seq of each chunk fact anchors
    replay order. Returns the stream entity id."""
    ent = _entity("pty-stream", id)
    parent = _entity("run", id) if kind != "spawn" else _entity("proc", id)
    conn._enqueue_facts([(ent, "type", "pty_stream"), (ent, "child_of", parent)])
    return ent


def stream_chunk(conn: WeaveStore, stream: str, data: bytes) -> None:
    """One flushed output range: bytes ride CAS, the stream gets exactly one
    (S, chunk, <hash>) fact (cardinality many, never deduped)."""
    if data:
        conn._enqueue_blob_fact(stream, "chunk", data)


def stream_snapshot(conn: WeaveStore, stream: str, data: bytes) -> None:
    """Periodic emulator-state snapshot: (S, snapshot, <hash>). Optional - the
    replay renderer treats a stream with no snapshots as replay-from-start."""
    if data:
        conn._enqueue_blob_fact(stream, "snapshot", data)


def finish(conn: WeaveStore, *, id: str, status: str, ended_at: float, output: str, result: str | None, error: str | None, error_line: int | None = None, outputs: list | None = None, bindings: dict | None = None, namespace: list | None = None) -> None:
    ent = _entity("run", id)
    ended = _ms(ended_at)
    facts: list[tuple[str, str, Any]] = [(ent, "status", status), (ent, "ended_ms", ended), (ent, "last_output", (output or "")[-200:]), (conn.agent, "last_output", (output or "")[-200:])]
    if result is not None:
        facts.append((ent, "result", _blob_text(conn, result)))
    if error is not None:
        facts.append((ent, "error", error))
    if error_line is not None:
        facts.append((ent, "error_line", error_line))
    if outputs is not None:
        facts.append((ent, "outputs", _blob(conn, outputs)))
    if bindings is not None:
        facts.append((ent, "bindings", _blob(conn, bindings)))
    if namespace is not None:
        facts.append((ent, "namespace", _blob(conn, namespace)))
    conn._enqueue_facts(facts)


def save_tool_view(conn: WeaveStore, *, id: str, html: str, label: str) -> str | None:
    """Persist one run's human HTML as a live weave view (weave2 n-toolviews).

    Fact shape is the cas-html view contract pinned in weave views.dl: type
    "view", renderer "cas-html", body = CAS hash of the html, label. Tool
    views link lineage with child_of the run entity - not from_msg, which is
    for chat-driven views answering a request message - so cascade cleanup
    and sidebar lineage follow the run. Returns the view entity id, or None
    when persistence is off (WEAVE_URL=off): the caller then attaches no
    view to the tool result."""
    if conn.disabled:
        return None
    ent = _entity("view", id)
    conn._enqueue_facts([
        (ent, "type", "view"),
        (ent, "renderer", "cas-html"),
        (ent, "body", _blob_text(conn, html)),
        (ent, "label", label),
        (ent, "child_of", _entity("run", id)),
    ])
    return ent


def recent(conn: WeaveStore, limit: int = 100) -> list[dict]:
    execs = _children(conn, ("run", "process"))
    out = [_exec_from_attrs(conn, ent, a) for ent, a in execs.items()]
    out.sort(key=lambda r: (r["status"] != "running", -(r["started_at"] or 0)))
    return out[:limit]


def latest_namespace(conn: WeaveStore) -> list[dict]:
    execs = _children(conn, ("run", "process"))
    done = [a for a in execs.values() if a.get("namespace") and a.get("ended_ms")]
    if not done:
        return []
    newest = max(done, key=lambda a: int(a.get("ended_ms") or 0))
    return _load_json_blob(conn, newest["namespace"], [])


def get(conn: WeaveStore, id: str) -> dict | None:
    for prefix in ("run", "proc"):
        ent = _entity(prefix, id)
        attrs = _attrs_of(conn, ent)
        if attrs:
            return _exec_from_attrs(conn, ent, attrs)
    return None


def get_session(conn: WeaveStore) -> dict | None:
    attrs = _attrs_of(conn, conn.agent)
    if not attrs:
        return None
    return {
        "name": attrs.get("label") or "",
        "client": attrs.get("client") or "",
        "updated_at": _sec(attrs.get("last_active_ms")) or _now(),
    }


def set_session(conn: WeaveStore, *, name: str, client: str) -> None:
    conn._enqueue_facts([(conn.agent, "label", name), (conn.agent, "client", client), (conn.agent, "session", name)])


def session_facts(conn: WeaveStore, *, id: str, status: str, client: str = "", connected_at: float | None = None) -> None:
    """One MCP connection's session entity (weave2 session contract, docs 4.6).

    status="connected" asserts the full shape; any other status re-asserts only
    the status fact: cardinality one, latest wins, never retracted (a closed
    session greys out on the board, it does not disappear). Re-asserting
    "connected" with the same connected_at is idempotent per attr (the
    write-behind queue drops unchanged values), so upgrading `client` once the
    initialize handshake names the real client emits just that one fact.
    """
    ent = _entity("session", id)
    if status != "connected":
        conn._enqueue_facts([(ent, "status", status)])
        return
    conn._enqueue_facts([
        (ent, "type", "session"),
        (ent, "child_of", conn.agent),
        (ent, "on_kernel", conn.kernel),
        (ent, "client", client),
        (ent, "connected_ms", _ms(connected_at)),
        (ent, "status", "connected"),
    ])


def cells(conn: WeaveStore) -> list[dict]:
    rows = _rows(conn.query(f"?- latest({conn.agent}, cells, C)."))
    return _load_json_blob(conn, rows[0].get("C") or "", []) if rows else []


def replace_cells(conn: WeaveStore, items: list[dict]) -> None:
    conn._enqueue_facts([(conn.agent, "cells", _blob(conn, items))])



def upsert_resource(conn: WeaveStore, *, id: str, title: str, kind: str, html: str, status: str, created_at: float, updated_at: float, execution_id: str = "") -> None:
    ent = _entity("resource", id)
    facts = [(ent, "type", "resource"), (ent, "child_of", conn.agent), (ent, "verb", kind), (ent, "label", title), (ent, "html", _blob_text(conn, html)), (ent, "status", status), (ent, "started_ms", _ms(created_at)), (ent, "last_active_ms", _ms(updated_at))]
    if execution_id:
        facts.append((ent, "of_run", _entity("run", execution_id)))
    conn._enqueue_facts(facts)


def close_resource(conn: WeaveStore, *, id: str, updated_at: float) -> None:
    conn._enqueue_facts([(_entity("resource", id), "status", "closed"), (_entity("resource", id), "ended_ms", _ms(updated_at))])


def open_channel(conn: WeaveStore, *, id: str, title: str) -> None:
    conn.mailbox("POST", "/api/mailbox/channels", json_body={"op": "open", "id": id, "title": title})


def close_channel(conn: WeaveStore, *, id: str) -> None:
    conn.mailbox("POST", "/api/mailbox/channels", json_body={"op": "close", "id": id})


def channel_open(conn: WeaveStore, id: str) -> bool:
    return bool(conn.mailbox("GET", f"/api/mailbox/channels/{quote(id, safe='')}").get("open"))


def add_input(conn: WeaveStore, *, channel: str, payload: str) -> None:
    conn.mailbox("POST", "/api/input", json_body={"channel": channel, "payload": json.loads(payload)})


def pending_inputs(conn: WeaveStore) -> list[dict]:
    # Non-consuming, exactly like the sqlite store: the kernel's drain calls
    # delete_inputs (delete-before-deliver, at-most-once) itself.
    return list(conn.mailbox("GET", "/api/mailbox/inputs") or [])


def delete_inputs(conn: WeaveStore, seqs: list[int]) -> None:
    if seqs:
        conn.mailbox("POST", "/api/mailbox/inputs/delete", json_body={"seqs": list(seqs)})


def add_outbox(conn: WeaveStore, *, content: str, meta: str, session: str = "") -> None:
    conn.mailbox("POST", "/api/mailbox/outbox", json_body={"content": content, "meta": meta, "session": session})


def take_outbox(conn: WeaveStore, *, session: str = "") -> list[dict]:
    from .mailbox import get_mailbox
    return get_mailbox().take_outbox(session=session)


def add_event(conn: WeaveStore, *, resource: str, kind: str, body: str) -> None:
    conn.mailbox("POST", "/api/mailbox/events", json_body={"resource": resource, "kind": kind, "body": body})


def latest_event_seq(conn: WeaveStore, resource: str) -> int:
    from .mailbox import get_mailbox
    return get_mailbox().latest_event_seq(resource)


def events_after(conn: WeaveStore, resource: str, seq: int) -> list[dict]:
    from .mailbox import get_mailbox
    return get_mailbox().events_after(resource, seq)


def resource_live(conn: WeaveStore, id: str) -> bool:
    if conn.disabled:
        # An empty answer would read as "dead"; without a journal the
        # question is unanswerable, so raise and let callers fail open.
        raise LookupError("weave persistence disabled; resource liveness unknowable")
    rows = _rows(conn.query(f"?- latest({_entity('resource', id)}, status, S)."))
    return bool(rows and rows[0].get("S") != "closed")


def save_snapshot(conn: WeaveStore, *, created_at: float, blob: bytes, names: list[str], skipped: list[dict]) -> None:
    h = conn.put_blob(blob)
    ent = f"snapshot:{h[:16]}"
    conn._enqueue_facts([(ent, "type", "snapshot"), (ent, "child_of", conn.agent), (ent, "created_ms", _ms(created_at)), (ent, "blob", h), (ent, "names", _blob(conn, names)), (ent, "skipped", _blob(conn, skipped)), (conn.agent, "snapshot", h)])


def latest_snapshot(conn: WeaveStore) -> dict | None:
    snaps = _pivot(conn, f"?- type(E, \"snapshot\"), child_of(E, {conn.agent}), latest(E, A, V).")
    if not snaps:
        return None
    _ent, attrs = max(snaps.items(), key=lambda kv: int(kv[1].get("created_ms") or 0))
    blob_hash = attrs.get("blob") or ""
    if not blob_hash:
        return None
    return {
        "created_at": _sec(attrs.get("created_ms")) or _now(),
        "blob": conn.get_blob(blob_hash),
        "names": _load_json_blob(conn, attrs.get("names") or "", []),
        "skipped": _load_json_blob(conn, attrs.get("skipped") or "", []),
    }


def mark_interrupted(conn: WeaveStore, *, ended_at: float) -> int:
    """Interrupt every running execution and close every live resource of
    this agent (the sqlite store's resume semantics, 1:1); the in-process
    mailbox (tier 3) resets wholesale."""
    ended = _ms(ended_at)
    children = _pivot(conn, f"?- child_of(E, {conn.agent}), latest(E, A, V).")
    facts: list[tuple[str, str, Any]] = []
    interrupted = 0
    for ent, attrs in children.items():
        typ = attrs.get("type")
        if typ in ("run", "process") and attrs.get("status") == "running":
            interrupted += 1
            facts.append((ent, "status", "interrupted"))
            facts.append((ent, "ended_ms", ended))
        elif typ == "resource" and attrs.get("status") != "closed":
            facts.append((ent, "status", "closed"))
            facts.append((ent, "ended_ms", ended))
    conn._enqueue_facts(facts)
    with contextlib.suppress(Exception):
        conn.mailbox("POST", "/api/mailbox/reset", json_body={})
    return interrupted


def replayable(conn: WeaveStore, since: float | None) -> list[dict]:
    execs = _children(conn, ("run",))
    out = []
    for ent, attrs in execs.items():
        if attrs.get("status") != "done" or (attrs.get("kind") or "cell") != "cell":
            continue
        item = _exec_from_attrs(conn, ent, attrs)
        if since is None or (item.get("ended_at") or 0) > since:
            out.append({"id": item["id"], "name": item["name"], "code": item["code"], "started_at": item["started_at"]})
    out.sort(key=lambda r: r.get("started_at") or 0)
    return [{"id": r["id"], "name": r["name"], "code": r["code"]} for r in out]


def live_resources(conn: WeaveStore) -> list[dict]:
    resources = _children(conn, ("resource",))
    out = []
    for ent, a in sorted(resources.items(), key=lambda kv: int(kv[1].get("started_ms") or 0)):
        if a.get("status") == "closed":
            continue
        out.append({
            "id": str(ent).split(":", 1)[-1],
            "title": a.get("label") or "",
            "kind": a.get("verb") or "html",
            "html": _load_text_blob(conn, a.get("html") or ""),
            "status": a.get("status") or "live",
            "execution_id": str(a.get("of_run") or "").split(":", 1)[-1],
            "created_at": _sec(a.get("started_ms")),
            "updated_at": _sec(a.get("last_active_ms")),
        })
    return out
