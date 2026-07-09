from __future__ import annotations

import json

import pytest

from ix_notebook_mcp.mailbox import Mailbox


def test_inputs_channel_lifecycle_and_consume() -> None:
    box = Mailbox()
    assert box.channel_open("cap") is False
    box.open_channel(id="cap", title="Name")
    assert box.channel_open("cap") is True
    box.add_input(channel="cap", payload=json.dumps({"value": "ada"}))
    box.add_input(channel="cap", payload=json.dumps({"value": "linus"}))
    rows = box.pending_inputs()
    assert [json.loads(row["payload"])["value"] for row in rows] == ["ada", "linus"]
    box.delete_inputs([rows[0]["seq"]])
    assert [json.loads(row["payload"])["value"] for row in box.pending_inputs()] == ["linus"]
    box.close_channel(id="cap")
    assert box.channel_open("cap") is False
    assert box.pending_inputs() == []


def test_input_payload_cap() -> None:
    box = Mailbox()
    with pytest.raises(ValueError):
        box.add_input(channel="cap", payload="x" * (256 * 1024 + 1))


def test_outbox_exactly_once_and_session_routing() -> None:
    box = Mailbox()
    box.add_outbox(content="broadcast", meta="{}")
    box.add_outbox(content="mine", meta="{}", session="s1")
    box.add_outbox(content="theirs", meta="{}", session="s2")
    assert [(r["content"], r["session"]) for r in box.take_outbox(session="s1")] == [("broadcast", ""), ("mine", "s1")]
    assert box.take_outbox(session="s1") == []
    assert [r["content"] for r in box.take_outbox(session="s2")] == ["theirs"]
    assert box.take_outbox(session="s2") == []


def test_events_after_latest_and_reset() -> None:
    box = Mailbox()
    assert box.latest_event_seq("res") == 0
    box.add_event(resource="res", kind="reply", body=json.dumps({"text": "hi"}))
    start = box.latest_event_seq("res")
    box.add_event(resource="other", kind="reply", body="{}")
    box.add_event(resource="res", kind="action_result", body=json.dumps({"value": 1}))
    rows = box.events_after("res", start)
    assert [row["kind"] for row in rows] == ["action_result"]
    box.reset()
    assert box.events_after("res", 0) == []
    assert box.take_outbox() == []
