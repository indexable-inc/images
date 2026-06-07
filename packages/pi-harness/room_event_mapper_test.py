#!/usr/bin/env python3
from __future__ import annotations

import json
import sqlite3
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


if __name__ == "__main__":
    unittest.main()
