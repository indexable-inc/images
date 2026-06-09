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

    def _run_lifecycle(self, events):
        emitter = CaptureEmitter()
        lifecycle = mapper.TurnLifecycle(emitter)  # type: ignore[arg-type]
        for event in events:
            lifecycle.handle(event)
        lifecycle.close()
        return emitter.events

    def test_single_attempt_emits_one_turn_completed_without_error(self) -> None:
        emitted = self._run_lifecycle(
            [
                {"type": "agent_start"},
                {"type": "turn_start"},
                {"type": "message_start"},
                {"type": "message_end", "message": {"stopReason": "stop"}},
                {"type": "turn_end"},
                {"type": "agent_end"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertNotIn("error", completed[0])
        self.assertNotIn("status", completed[0])
        # The terminal turn_completed lands before agent_end, as in the raw stream.
        self.assertEqual([event["pi_type"] for event in emitted[-2:]], ["turn_end", "agent_end"])

    def test_retried_then_succeeded_emits_one_terminal_turn_completed(self) -> None:
        emitted = self._run_lifecycle(
            [
                {"type": "agent_start"},
                {"type": "turn_start"},
                {"type": "message_start"},
                {"type": "message_end", "message": {"stopReason": "error", "errorMessage": "overloaded"}},
                {"type": "turn_end"},
                {"type": "agent_end", "willRetry": True},
                {"type": "auto_retry_start", "attempt": 2},
                {"type": "turn_start"},
                {"type": "message_start"},
                {"type": "message_end", "message": {"stopReason": "stop"}},
                {"type": "turn_end"},
                {"type": "agent_end"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertNotIn("error", completed[0])
        self.assertNotIn("status", completed[0])
        # The first attempt's turn_end is suppressed entirely; the retry
        # bookkeeping events still pass through for observability.
        self.assertIn("auto_retry_start", [event.get("pi_type") for event in emitted])

    def test_all_attempts_failed_emits_one_turn_completed_with_error(self) -> None:
        failed_attempt = [
            {"type": "turn_start"},
            {"type": "message_start"},
            {"type": "message_end", "message": {"stopReason": "error", "errorMessage": "overloaded"}},
            {"type": "turn_end"},
        ]
        emitted = self._run_lifecycle(
            [{"type": "agent_start"}]
            + failed_attempt
            + [{"type": "agent_end", "willRetry": True}, {"type": "auto_retry_start", "attempt": 2}]
            + failed_attempt
            + [{"type": "agent_end", "willRetry": True}, {"type": "auto_retry_start", "attempt": 3}]
            + failed_attempt
            + [{"type": "agent_end"}]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["status"], "error")
        self.assertEqual(completed[0]["error"], "overloaded")

    def test_stream_ending_after_turn_end_still_flushes(self) -> None:
        # No agent_end (pi crashed or stream cut): close() must still emit the
        # held turn_completed so Room sees the turn terminate.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "message_end", "stopReason": "error", "errorMessage": "boom"},
                {"type": "turn_end"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["error"], "boom")

    def test_suppressed_attempt_usage_lands_on_terminal_event(self) -> None:
        # Usage billed for a suppressed retry attempt must not vanish: it rides
        # on the terminal turn_completed under retried_usage, separate from the
        # final attempt's own usage so consumers cannot double count.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "message_end", "message": {"stopReason": "error", "errorMessage": "overloaded"}},
                {"type": "turn_end", "usage": {"input": 10, "output": 1}},
                {"type": "agent_end", "willRetry": True},
                {"type": "auto_retry_start", "attempt": 2},
                {"type": "turn_start"},
                {"type": "turn_end", "usage": {"input": 12, "output": 40}},
                {"type": "agent_end"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["usage"], {"input": 12, "output": 40})
        self.assertEqual(completed[0]["retried_usage"], [{"input": 10, "output": 1}])
        self.assertNotIn("error", completed[0])

    def test_fallback_event_does_not_double_count_its_own_usage(self) -> None:
        # Two suppressed attempts, then the stream dies: the fallback terminal
        # event is the second attempt's turn_end, so retried_usage keeps only
        # the first attempt's usage.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "turn_end", "usage": {"input": 10}},
                {"type": "agent_end", "willRetry": True},
                {"type": "turn_start"},
                {"type": "turn_end", "usage": {"input": 20}},
                {"type": "agent_end", "willRetry": True},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["usage"], {"input": 20})
        self.assertEqual(completed[0]["retried_usage"], [{"input": 10}])
        self.assertEqual(completed[0]["status"], "error")

    def test_stream_cut_after_retry_announcement_still_terminates_turn(self) -> None:
        # willRetry suppressed the attempt's turn_end, then the stream died
        # before the next attempt produced one: the suppressed attempt must
        # surface as the failed terminal event so Room sees the turn end.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "message_end", "message": {"stopReason": "error", "errorMessage": "overloaded"}},
                {"type": "turn_end"},
                {"type": "agent_end", "willRetry": True},
                {"type": "auto_retry_start", "attempt": 2},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["status"], "error")
        self.assertEqual(completed[0]["error"], "overloaded")
        self.assertEqual(emitted[-1], completed[0])

    def test_stream_cut_during_second_attempt_uses_suppressed_fallback(self) -> None:
        # The retry started (turn_start arrived) but died before its turn_end:
        # the suppressed first attempt still terminates the turn.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "message_end", "message": {"stopReason": "error", "errorMessage": "overloaded"}},
                {"type": "turn_end"},
                {"type": "agent_end", "willRetry": True},
                {"type": "auto_retry_start", "attempt": 2},
                {"type": "turn_start"},
                {"type": "message_start"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 1)
        self.assertEqual(completed[0]["status"], "error")
        self.assertEqual(completed[0]["error"], "overloaded")

    def test_multi_turn_run_keeps_each_turn_completed(self) -> None:
        # Consecutive turns in one agent run are real completions, not retries.
        emitted = self._run_lifecycle(
            [
                {"type": "turn_start"},
                {"type": "turn_end"},
                {"type": "turn_start"},
                {"type": "turn_end"},
                {"type": "agent_end"},
            ]
        )
        completed = [event for event in emitted if event["type"] == "turn_completed"]
        self.assertEqual(len(completed), 2)
        self.assertNotIn("error", completed[0])

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
