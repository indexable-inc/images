"""Tier-3 in-process mailbox for interactive channels and transient events.

This module is intentionally not durable truth. It is the serve-process cache for
browser inputs, MCP outbox notifications, and resource event streams; Weave facts
remain the durable tier for notebook state.
"""

from __future__ import annotations

import time
from dataclasses import dataclass, field
from threading import RLock

_EVENT_MAX_AGE_SECONDS = 3600.0
_OUTBOX_MAX_AGE_SECONDS = 3600.0
_INPUT_MAX_BYTES = 256 * 1024


def _now() -> float:
    return time.time()


@dataclass
class Mailbox:
    _lock: RLock = field(default_factory=RLock)
    _channels: dict[str, dict] = field(default_factory=dict)
    _inputs: list[dict] = field(default_factory=list)
    _outbox: list[dict] = field(default_factory=list)
    _events: list[dict] = field(default_factory=list)
    _input_seq: int = 0
    _outbox_seq: int = 0
    _event_seq: int = 0

    def reset(self) -> None:
        with self._lock:
            self._channels.clear()
            self._inputs.clear()
            self._outbox.clear()
            self._events.clear()

    def open_channel(self, *, id: str, title: str) -> None:
        now = _now()
        with self._lock:
            created = self._channels.get(id, {}).get("created_at", now)
            self._channels[id] = {
                "id": id,
                "title": title,
                "status": "open",
                "created_at": created,
                "updated_at": now,
            }

    def close_channel(self, *, id: str) -> None:
        now = _now()
        with self._lock:
            row = self._channels.setdefault(
                id, {"id": id, "title": "", "created_at": now}
            )
            row["status"] = "closed"
            row["updated_at"] = now
            self._inputs = [item for item in self._inputs if item["channel"] != id]

    def channel_open(self, id: str) -> bool:
        with self._lock:
            return self._channels.get(id, {}).get("status") == "open"

    def add_input(self, *, channel: str, payload: str) -> None:
        if len(payload.encode("utf-8")) > _INPUT_MAX_BYTES:
            raise ValueError("input payload exceeds 256 KiB")
        with self._lock:
            self._input_seq += 1
            self._inputs.append(
                {"seq": self._input_seq, "channel": channel, "payload": payload, "created_at": _now()}
            )

    def pending_inputs(self) -> list[dict]:
        with self._lock:
            return [dict(row) for row in sorted(self._inputs, key=lambda r: r["seq"])]

    def delete_inputs(self, seqs: list[int]) -> None:
        seqset = set(seqs)
        with self._lock:
            self._inputs = [row for row in self._inputs if row["seq"] not in seqset]

    def _prune_outbox(self, now: float) -> None:
        cutoff = now - _OUTBOX_MAX_AGE_SECONDS
        self._outbox = [row for row in self._outbox if row["created_at"] >= cutoff]

    def add_outbox(self, *, content: str, meta: str, session: str = "") -> None:
        now = _now()
        with self._lock:
            self._prune_outbox(now)
            self._outbox_seq += 1
            self._outbox.append(
                {"seq": self._outbox_seq, "content": content, "meta": meta, "session": session, "created_at": now}
            )

    def take_outbox(self, *, session: str = "") -> list[dict]:
        with self._lock:
            wanted = [row for row in self._outbox if row.get("session", "") in ("", session)]
            wanted_seqs = {row["seq"] for row in wanted}
            self._outbox = [row for row in self._outbox if row["seq"] not in wanted_seqs]
            return [dict(row) for row in sorted(wanted, key=lambda r: r["seq"])]

    def _prune_events(self, now: float) -> None:
        cutoff = now - _EVENT_MAX_AGE_SECONDS
        self._events = [row for row in self._events if row["created_at"] >= cutoff]

    def add_event(self, *, resource: str, kind: str, body: str) -> None:
        now = _now()
        with self._lock:
            self._prune_events(now)
            self._event_seq += 1
            self._events.append(
                {"seq": self._event_seq, "resource": resource, "kind": kind, "body": body, "created_at": now}
            )

    def latest_event_seq(self, resource: str) -> int:
        with self._lock:
            seqs = [row["seq"] for row in self._events if row["resource"] == resource]
            return max(seqs) if seqs else 0

    def events_after(self, resource: str, seq: int) -> list[dict]:
        with self._lock:
            rows = [row for row in self._events if row["resource"] == resource and row["seq"] > seq]
            return [dict(row) for row in sorted(rows, key=lambda r: r["seq"])]


_default = Mailbox()


def get_mailbox() -> Mailbox:
    return _default


def reset() -> None:
    _default.reset()
