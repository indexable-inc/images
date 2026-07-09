"""A hermetic in-memory double of the Weave HTTP ABI for store.py tests.

This is NOT a datalog engine: it interprets exactly the query shapes
`ix_notebook_mcp.store` issues (latest-wins pivots over one agent's
children and single-entity attribute lookups). Fidelity against the real
server is pinned separately by the WEAVE_BIN-gated integration test; unit
tests stay hermetic and fast through this stub.
"""

from __future__ import annotations

import hashlib
import json
import re
from typing import Any

_PIVOT_SNAPSHOT = re.compile(r'^\?- type\(E, "snapshot"\), child_of\(E, (\S+)\), latest\(E, A, V\)\.$')
_PIVOT_CHILDREN = re.compile(r"^\?- child_of\(E, (\S+)\), latest\(E, A, V\)\.$")
_ATTRS_OF = re.compile(r"^\?- latest\((\S+), A, V\)\.$")
_ONE_ATTR = re.compile(r"^\?- latest\((\S+), (\w+), \w+\)\.$")


def _tag(value: Any) -> dict:
    if isinstance(value, dict) and "t" in value:
        return value
    if isinstance(value, bool):
        return {"t": "bool", "v": value}
    if isinstance(value, int):
        return {"t": "int", "v": value}
    if isinstance(value, float):
        return {"t": "float", "v": value}
    return {"t": "str", "v": str(value)}


class FakeWeave:
    def __init__(self) -> None:
        self.facts: dict[tuple[str, str], Any] = {}
        self.blobs: dict[str, bytes] = {}
        self.writes: list[dict] = []
        self.seq = 0

    # -- write side -------------------------------------------------------
    def _apply(self, item: dict) -> dict:
        self.writes.append(item)
        fact = item.get("fact")
        if fact:
            entity = fact["entity"]["v"] if isinstance(fact["entity"], dict) else fact["entity"]
            value = fact["value"]["v"] if isinstance(fact["value"], dict) else fact["value"]
            self.facts[(str(entity), str(fact["attr"]))] = value
        self.seq += 1
        return {"seq": self.seq, "id": f"{self.seq:064d}"[:64]}

    # -- read side ----------------------------------------------------------
    def _attrs(self, entity: str) -> dict[str, Any]:
        return {a: v for (e, a), v in self.facts.items() if e == entity}

    def _children_of(self, parent: str) -> list[str]:
        return [e for (e, a), v in self.facts.items() if a == "child_of" and v == parent]

    def query(self, program: str) -> dict:
        program = program.strip()
        m = _PIVOT_SNAPSHOT.match(program)
        if m:
            rows = [
                [_tag(e), _tag(a), _tag(v)]
                for e in self._children_of(m.group(1))
                if self.facts.get((e, "type")) == "snapshot"
                for a, v in self._attrs(e).items()
            ]
            return {"vars": ["E", "A", "V"], "rows": rows, "as_of": self.seq}
        m = _PIVOT_CHILDREN.match(program)
        if m:
            rows = [
                [_tag(e), _tag(a), _tag(v)]
                for e in self._children_of(m.group(1))
                for a, v in self._attrs(e).items()
            ]
            return {"vars": ["E", "A", "V"], "rows": rows, "as_of": self.seq}
        m = _ATTRS_OF.match(program)
        if m:
            rows = [[_tag(a), _tag(v)] for a, v in self._attrs(m.group(1)).items()]
            return {"vars": ["A", "V"], "rows": rows, "as_of": self.seq}
        m = _ONE_ATTR.match(program)
        if m:
            value = self.facts.get((m.group(1), m.group(2)))
            rows = [[_tag(value)]] if value is not None else []
            return {"vars": ["S"], "rows": rows, "as_of": self.seq}
        raise AssertionError(f"weave_stub: unhandled query shape: {program!r}")

    # -- transport hooks ------------------------------------------------------
    def http_json(self, method: str, url: str, *, body: Any = None, content: bytes | None = None) -> Any:
        if url.endswith("/api/facts"):
            items = body if isinstance(body, list) else [body]
            acks = [self._apply(item) for item in items]
            return acks if isinstance(body, list) else acks[0]
        if url.endswith("/api/blob"):
            digest = hashlib.sha256(content or b"").hexdigest()
            self.blobs[digest] = content or b""
            return {"hash": digest}
        if url.endswith("/api/query"):
            return self.query(str(body["program"]))
        if "/api/mailbox/" in url or url.endswith("/api/input"):
            raise ConnectionError("no data api in unit tests (mailbox falls back in-process)")
        raise AssertionError(f"weave_stub: unhandled url {url}")

    def http_bytes(self, method: str, url: str, *, content: bytes | None = None) -> bytes:
        digest = url.rsplit("/", 1)[-1]
        return self.blobs.get(digest, b"")


def install(monkeypatch) -> FakeWeave:
    """Point ix_notebook_mcp.store at an in-memory Weave for this test."""
    from ix_notebook_mcp import store

    fake = FakeWeave()
    monkeypatch.setenv("WEAVE_URL", "http://weave.stub")
    monkeypatch.setattr(store, "_http_json", fake.http_json)
    monkeypatch.setattr(store, "_http_bytes", fake.http_bytes)
    return fake


__all__ = ["FakeWeave", "install"]
