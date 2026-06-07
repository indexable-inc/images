#!/usr/bin/env python3
"""Map Pi JSON plus ix-mcp dashboard rows into Room-facing engine events.

Pi owns the model turn and emits generic JSON lifecycle/tool events. ix-mcp owns
the notebook state and persists the same Job/Cell/Resource rows its Svelte
dashboard already renders. Room needs one ordered JSON stream that keeps both
surfaces explicit, so this mapper passes Pi lifecycle through under stable Room
event names while polling the MCP store for dashboard-shaped updates.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import sqlite3
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any


Json = dict[str, Any]

PI_TO_ROOM_EVENTS = {
    "turn_start": "turn_started",
    "turn_started": "turn_started",
    "message_update": "text_delta",
    "assistant_message_delta": "text_delta",
    "assistantMessageEvent": "text_delta",
    "reasoning_delta": "reasoning_delta",
    "thinking_delta": "reasoning_delta",
    "tool_execution_start": "tool_call_started",
    "tool_call_started": "tool_call_started",
    "tool_execution_update": "tool_call_output",
    "tool_execution_end": "tool_call_output",
    "tool_call_output": "tool_call_output",
    "usage": "usage",
    "turn_end": "turn_completed",
    "turn_completed": "turn_completed",
    "error": "error",
}


def _json_dumps(event: Json) -> str:
    return json.dumps(event, ensure_ascii=False, separators=(",", ":"))


class Emitter:
    def __init__(self) -> None:
        self._lock = threading.Lock()

    def emit(self, event: Json) -> None:
        with self._lock:
            print(_json_dumps(event), flush=True)


def _event_type(event: Json) -> str | None:
    value = event.get("type") or event.get("event") or event.get("name")
    if isinstance(value, str):
        return value
    nested = event.get("assistantMessageEvent")
    if isinstance(nested, dict):
        nested_type = nested.get("type")
        if isinstance(nested_type, str):
            return nested_type
    return None


def _text_delta(event: Json) -> str | None:
    for key in ("delta", "text", "content"):
        value = event.get(key)
        if isinstance(value, str):
            return value

    nested = event.get("assistantMessageEvent")
    if isinstance(nested, dict):
        for key in ("delta", "text", "content"):
            value = nested.get(key)
            if isinstance(value, str):
                return value
    return None


def map_pi_event(event: Json) -> Json:
    pi_type = _event_type(event)
    room_type = PI_TO_ROOM_EVENTS.get(pi_type or "", "pi_event")
    mapped: Json = {"type": room_type, "source": "pi", "raw": event}

    if pi_type is not None:
        mapped["pi_type"] = pi_type

    if room_type in {"text_delta", "reasoning_delta"}:
        delta = _text_delta(event)
        if delta is not None:
            mapped["delta"] = delta

    for key in ("toolCallId", "tool_call_id", "id", "name", "usage", "error"):
        if key in event:
            mapped[key] = event[key]

    return mapped


def _connect(path: Path) -> sqlite3.Connection | None:
    if not path.exists():
        return None
    try:
        conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True, timeout=0.2)
        conn.row_factory = sqlite3.Row
        return conn
    except sqlite3.Error:
        return None


def _load_json(value: Any, fallback: Any) -> Any:
    if not isinstance(value, str) or value == "":
        return fallback
    try:
        return json.loads(value)
    except json.JSONDecodeError:
        return fallback


def _rows(conn: sqlite3.Connection, query: str) -> list[sqlite3.Row]:
    try:
        return list(conn.execute(query).fetchall())
    except sqlite3.Error:
        return []


def _job_from_row(row: sqlite3.Row) -> Json:
    status = row["status"]
    return {
        "id": row["id"],
        "name": row["name"],
        "code": row["code"],
        "code_html": "",
        "status": status,
        "started_at": row["started_at"],
        "ended_at": row["ended_at"],
        "output": row["output"],
        "result": row["result"],
        "error": row["error"],
        "outputs": _load_json(row["outputs"], []),
        "bindings": _load_json(row["bindings"], {}),
    }


def _execution_event(row: sqlite3.Row) -> Json:
    job = _job_from_row(row)
    return {
        "type": "cell_update",
        "source": "ix-mcp",
        "cell_kind": "execution",
        "id": job["id"],
        "job": job,
    }


def _cell_from_row(row: sqlite3.Row) -> Json:
    return {
        "id": row["id"],
        "title": row["title"],
        "position": row["position"],
        "outputs": _load_json(row["outputs"], []),
        "updated_at": row["updated_at"],
    }


def _presentation_cell_event(row: sqlite3.Row) -> Json:
    cell = _cell_from_row(row)
    return {
        "type": "cell_update",
        "source": "ix-mcp",
        "cell_kind": "presentation",
        "id": cell["id"],
        "cell": cell,
    }


def _resource_from_row(row: sqlite3.Row) -> Json:
    return {
        "id": row["id"],
        "title": row["title"],
        "kind": row["kind"],
        "html": row["html"],
        "status": row["status"],
        "created_at": row["created_at"],
        "updated_at": row["updated_at"],
    }


def _resource_event(row: sqlite3.Row) -> Json:
    resource = _resource_from_row(row)
    return {
        "type": "resource_update",
        "source": "ix-mcp",
        "id": resource["id"],
        "resource": resource,
    }


class StorePoller:
    def __init__(self, store: Path, interval: float, emitter: Emitter) -> None:
        self._store = store
        self._interval = interval
        self._emitter = emitter
        self._seen: dict[str, str] = {}
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, name="ix-mcp-store-poller", daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=max(1.0, self._interval * 4))
        self.poll_once()

    def _run(self) -> None:
        while not self._stop.wait(self._interval):
            self.poll_once()

    def poll_once(self) -> None:
        conn = _connect(self._store)
        if conn is None:
            return
        try:
            events: list[Json] = []
            for row in _rows(
                conn,
                "SELECT id, name, code, status, started_at, ended_at, output, result, error, outputs, bindings "
                "FROM executions ORDER BY started_at ASC",
            ):
                events.append(_execution_event(row))
            for row in _rows(
                conn,
                "SELECT id, title, position, outputs, updated_at FROM cells ORDER BY position ASC",
            ):
                events.append(_presentation_cell_event(row))
            for row in _rows(
                conn,
                "SELECT id, title, kind, html, status, created_at, updated_at FROM resources ORDER BY created_at ASC",
            ):
                events.append(_resource_event(row))
        finally:
            conn.close()

        for event in events:
            key = f"{event['type']}:{event.get('cell_kind', '')}:{event['id']}"
            encoded = _json_dumps(event)
            if self._seen.get(key) == encoded:
                continue
            self._seen[key] = encoded
            self._emitter.emit(event)


def _reader(lines: queue.Queue[str | None]) -> None:
    for line in sys.stdin:
        lines.put(line)
    lines.put(None)


def _read_stream(stream, lines: queue.Queue[str | None]) -> None:
    for line in stream:
        lines.put(line)
    lines.put(None)


def _strip_command_separator(command: list[str]) -> list[str]:
    if command and command[0] == "--":
        return command[1:]
    return command


def run(store: Path, interval: float, command: list[str] | None = None) -> int:
    emitter = Emitter()
    poller = StorePoller(store, interval, emitter)
    poller.start()

    lines: queue.Queue[str | None] = queue.Queue()
    process: subprocess.Popen[str] | None = None
    if command:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
        assert process.stdout is not None
        threading.Thread(target=_read_stream, args=(process.stdout, lines), name="pi-stdout-reader", daemon=True).start()
    else:
        threading.Thread(target=_reader, args=(lines,), name="stdin-reader", daemon=True).start()

    try:
        while True:
            poller.poll_once()
            try:
                line = lines.get(timeout=interval)
            except queue.Empty:
                continue
            if line is None:
                break
            stripped = line.strip()
            if not stripped:
                continue
            try:
                event = json.loads(stripped)
            except json.JSONDecodeError as exc:
                emitter.emit({"type": "error", "source": "pi-harness", "message": str(exc), "line": stripped})
                continue
            if isinstance(event, dict):
                emitter.emit(map_pi_event(event))
            else:
                emitter.emit({"type": "pi_event", "source": "pi", "raw": event})
    finally:
        poller.stop()
    if process is None:
        return 0
    return process.wait()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", default=os.environ.get("IX_MCP_STORE"), help="ix-mcp SQLite store path")
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=float(os.environ.get("PI_HARNESS_MCP_POLL_INTERVAL", "0.2")),
        help="seconds between ix-mcp store polls",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER, help="optional Pi command to spawn and map")
    args = parser.parse_args()
    if not args.store:
        print("room-event-mapper: --store or IX_MCP_STORE is required", file=sys.stderr)
        return 2
    command = _strip_command_separator(args.command)
    return run(Path(args.store), max(args.poll_interval, 0.05), command or None)


if __name__ == "__main__":
    raise SystemExit(main())
