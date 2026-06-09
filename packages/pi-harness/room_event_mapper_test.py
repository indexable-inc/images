#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path

import room_event_mapper as mapper


SCHEMA = """
CREATE TABLE executions (
    id TEXT PRIMARY KEY,
    name TEXT,
    code TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at REAL NOT NULL,
    ended_at REAL,
    output TEXT NOT NULL DEFAULT '',
    result TEXT,
    error TEXT,
    outputs TEXT NOT NULL DEFAULT '[]',
    bindings TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE cells (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    position INTEGER NOT NULL,
    outputs TEXT NOT NULL DEFAULT '[]',
    updated_at REAL NOT NULL
);
CREATE TABLE resources (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'html',
    html TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'live',
    created_at REAL NOT NULL,
    updated_at REAL NOT NULL
);
"""


class CaptureEmitter:
    def __init__(self) -> None:
        self.events = []

    def emit(self, event):
        self.events.append(event)


class MapperTest(unittest.TestCase):
    def test_maps_pi_lifecycle_events(self) -> None:
        self.assertEqual(mapper.map_pi_event({"type": "turn_start"})["type"], "turn_started")
        text = mapper.map_pi_event({"type": "message_update", "delta": "hi"})
        self.assertEqual(text["type"], "text_delta")
        self.assertEqual(text["delta"], "hi")
        text_start = mapper.map_pi_event(
            {"type": "message_update", "assistantMessageEvent": {"type": "text_start"}}
        )
        self.assertEqual(text_start["type"], "pi_event")
        text_delta = mapper.map_pi_event(
            {"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "hi"}}
        )
        self.assertEqual(text_delta["type"], "text_delta")
        self.assertEqual(text_delta["delta"], "hi")
        text_end = mapper.map_pi_event(
            {"type": "message_update", "assistantMessageEvent": {"type": "text_end", "delta": "hi"}}
        )
        self.assertEqual(text_end["type"], "pi_event")
        self.assertEqual(mapper.map_pi_event({"type": "tool_execution_start", "id": "t1"})["type"], "tool_call_started")
        self.assertEqual(mapper.map_pi_event({"type": "turn_end"})["type"], "turn_completed")

    def test_strips_command_separator(self) -> None:
        self.assertEqual(mapper._strip_command_separator(["--", "pi", "--mode", "json"]), ["pi", "--mode", "json"])
        self.assertEqual(mapper._strip_command_separator(["pi"]), ["pi"])

    def test_polls_cells_and_resources_from_store(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp) / "mcp.sqlite"
            conn = sqlite3.connect(store)
            conn.executescript(SCHEMA)
            conn.execute(
                "INSERT INTO executions VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    "job-1",
                    "demo",
                    "Result.text('ok')",
                    "done",
                    1.0,
                    2.0,
                    "stdout",
                    "ok",
                    None,
                    json.dumps([{"type": "text", "text": "ok"}]),
                    json.dumps({"value": {"repr": "1"}}),
                ),
            )
            conn.execute(
                "INSERT INTO cells VALUES (?, ?, ?, ?, ?)",
                ("cell-1", "Answer", 0, json.dumps([{"type": "html", "html": "<b>ok</b>"}]), 3.0),
            )
            conn.execute(
                "INSERT INTO resources VALUES (?, ?, ?, ?, ?, ?, ?)",
                ("res-1", "Terminal", "html", "<pre>hi</pre>", "live", 4.0, 5.0),
            )
            conn.commit()
            conn.close()

            emitter = CaptureEmitter()
            poller = mapper.StorePoller(store, 0.1, emitter)  # type: ignore[arg-type]
            poller.poll_once()
            self.assertEqual([event["type"] for event in emitter.events], ["cell_update", "cell_update", "resource_update"])
            execution = emitter.events[0]
            self.assertEqual(execution["cell_kind"], "execution")
            self.assertEqual(execution["job"]["code"], "Result.text('ok')")
            self.assertEqual(execution["job"]["code_html"], "")
            self.assertEqual(execution["job"]["outputs"][0]["text"], "ok")
            self.assertEqual(emitter.events[1]["cell_kind"], "presentation")
            self.assertEqual(emitter.events[1]["cell"]["title"], "Answer")
            self.assertEqual(emitter.events[2]["resource"]["html"], "<pre>hi</pre>")

            poller.poll_once()
            self.assertEqual(len(emitter.events), 3)

    def test_polls_removed_presentation_cell(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp) / "mcp.sqlite"
            conn = sqlite3.connect(store)
            conn.executescript(SCHEMA)
            conn.execute(
                "INSERT INTO cells VALUES (?, ?, ?, ?, ?)",
                ("cell-removed", "Stale", 0, json.dumps([]), 1.0),
            )
            conn.commit()
            conn.close()

            emitter = CaptureEmitter()
            poller = mapper.StorePoller(store, 0.1, emitter)  # type: ignore[arg-type]
            poller.poll_once()
            self.assertEqual(emitter.events[-1]["cell"]["id"], "cell-removed")

            conn = sqlite3.connect(store)
            conn.execute("DELETE FROM cells WHERE id = ?", ("cell-removed",))
            conn.commit()
            conn.close()

            poller.poll_once()
            removed = emitter.events[-1]
            self.assertEqual(removed["type"], "cell_update")
            self.assertEqual(removed["cell_kind"], "presentation")
            self.assertEqual(removed["id"], "cell-removed")
            self.assertTrue(removed["removed"])
            self.assertTrue(removed["cell"]["removed"])

    def test_cell_query_failure_does_not_emit_removals(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp) / "mcp.sqlite"
            conn = sqlite3.connect(store)
            conn.executescript(SCHEMA)
            conn.execute(
                "INSERT INTO cells VALUES (?, ?, ?, ?, ?)",
                ("cell-kept", "Kept", 0, json.dumps([]), 1.0),
            )
            conn.commit()
            conn.close()

            emitter = CaptureEmitter()
            poller = mapper.StorePoller(store, 0.1, emitter)  # type: ignore[arg-type]
            poller.poll_once()
            self.assertEqual(emitter.events[-1]["cell"]["id"], "cell-kept")

            conn = sqlite3.connect(store)
            conn.execute("DROP TABLE cells")
            conn.commit()
            conn.close()

            before = len(emitter.events)
            poller.poll_once()
            self.assertEqual(len(emitter.events), before)

    def test_spawned_command_gets_store_env(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp) / "mcp.sqlite"
            env_seen = Path(tmp) / "env-seen.txt"
            old = os.environ.pop("IX_MCP_STORE", None)
            old_env_seen = os.environ.get("ENV_SEEN")
            os.environ["ENV_SEEN"] = str(env_seen)
            try:
                rc = mapper.run(
                    store,
                    0.05,
                    [
                        sys.executable,
                        "-c",
                        "import os, pathlib; pathlib.Path(os.environ['ENV_SEEN']).write_text(os.environ.get('IX_MCP_STORE', ''))",
                    ],
                )
            finally:
                if old is not None:
                    os.environ["IX_MCP_STORE"] = old
                else:
                    os.environ.pop("IX_MCP_STORE", None)
                if old_env_seen is not None:
                    os.environ["ENV_SEEN"] = old_env_seen
                else:
                    os.environ.pop("ENV_SEEN", None)
            self.assertEqual(rc, 0)
            self.assertEqual(env_seen.read_text(), str(store))
            self.assertEqual(os.environ.get("IX_MCP_STORE"), old)


if __name__ == "__main__":
    unittest.main()
