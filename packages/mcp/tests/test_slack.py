"""Network-free tests for the `slack` helper.

These never reach Slack: they check the module's shape (exports, explicit type
hints) and that `send` builds the right `chat.postMessage` params for top-level
posts vs. in-thread replies, by stubbing the one network primitive
(`_api_call`) and a token.
"""

from __future__ import annotations

import asyncio
import inspect
import sys
from pathlib import Path
from typing import Any

import pytest

# Prefer the bundled module (nix check); fall back to the source tree (dev run).
SLACK_SRC = Path(__file__).resolve().parents[1] / "src" / "slack"
if SLACK_SRC.is_dir() and str(SLACK_SRC) not in sys.path:
    sys.path.insert(0, str(SLACK_SRC))

import slack

# Public callables = everything exported except the error classes.
_PUBLIC_FUNCS = [
    obj
    for name in slack.__all__
    if not (isinstance(obj := getattr(slack, name), type) and issubclass(obj, BaseException))
]

# A channel id resolves without any API call (no _resolve_channel network hop).
_CHANNEL_ID = "C0123456789"
_PARENT_TS = "1781738574.768059"


def test_all_names_exist() -> None:
    for name in slack.__all__:
        assert hasattr(slack, name), f"{name} in __all__ but missing from module"


def test_error_type() -> None:
    assert issubclass(slack.SlackError, RuntimeError)


def test_type_hints_explicit() -> None:
    # Mirrors the ruff ANN gate: every public function fully annotates its params
    # and return type.
    for func in _PUBLIC_FUNCS:
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty, (
            f"{func.__name__} missing return annotation"
        )
        for pname, param in sig.parameters.items():
            assert param.annotation is not inspect.Parameter.empty, (
                f"{func.__name__}({pname}) missing annotation"
            )


@pytest.fixture
def stub_slack(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, dict[str, Any]]]:
    """Stub the token + the one network primitive; capture (method, params)."""
    monkeypatch.setenv("SLACK_USER_TOKEN", "xoxp-test")
    monkeypatch.delenv(slack.SHARED_ENV, raising=False)
    calls: list[tuple[str, dict[str, Any]]] = []

    def fake_api(method: str, token: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        calls.append((method, params or {}))
        # Echo a threaded reply when thread_ts was sent, like Slack does.
        message = {"thread_ts": (params or {}).get("thread_ts", "")}
        return {"ok": True, "ts": "1781738999.000100", "channel": _CHANNEL_ID, "message": message}

    monkeypatch.setattr(slack, "_api_call", fake_api)
    return calls


def test_send_top_level_omits_thread_ts(stub_slack: list[tuple[str, dict[str, Any]]]) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "hello", seed_thread=False, watch=False))
    method, params = stub_slack[-1]
    assert method == "chat.postMessage"
    assert "thread_ts" not in params
    assert out["thread_ts"] == ""
    assert out["ok"] is True
    assert out["watching"] is False


def test_send_in_thread_passes_thread_ts(stub_slack: list[tuple[str, dict[str, Any]]]) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "reply", thread_ts=_PARENT_TS, watch=False))
    _, params = stub_slack[-1]
    assert params["thread_ts"] == _PARENT_TS
    assert "reply_broadcast" not in params
    assert out["thread_ts"] == _PARENT_TS


def test_send_reply_broadcast_sets_flag(stub_slack: list[tuple[str, dict[str, Any]]]) -> None:
    asyncio.run(
        slack.send(
            _CHANNEL_ID, "loud reply", thread_ts=_PARENT_TS, reply_broadcast=True, watch=False
        )
    )
    _, params = stub_slack[-1]
    assert params["thread_ts"] == _PARENT_TS
    assert params["reply_broadcast"] == "true"


def test_send_reply_broadcast_without_thread_ts_raises(
    stub_slack: list[tuple[str, dict[str, Any]]],
) -> None:
    with pytest.raises(slack.SlackError, match="reply_broadcast"):
        asyncio.run(slack.send(_CHANNEL_ID, "oops", reply_broadcast=True))
    # No network call should have been made before the guard fired.
    assert stub_slack == []


# --- thread watching ---------------------------------------------------------

_SELF_USER = "U0SELF00000"


@pytest.fixture
def fresh_watch_state(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, dict[str, str]]]:
    """Reset module watch state and route notify() into a recorder."""
    monkeypatch.setattr(slack, "_watches", {})
    monkeypatch.setattr(slack, "_watcher_task", None)
    monkeypatch.setattr(slack, "_self_user_id", None)
    delivered: list[tuple[str, dict[str, str]]] = []

    async def record(content: str, **meta: str) -> None:
        delivered.append((content, {k: str(v) for k, v in meta.items()}))

    monkeypatch.setattr(slack, "_resolve_notify", lambda: record)
    return delivered


@pytest.fixture
def threaded_api(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, dict[str, Any]]]:
    """Token + api stub with distinct, increasing ts per post and canned replies."""
    monkeypatch.setenv("SLACK_USER_TOKEN", "xoxp-test")
    monkeypatch.delenv(slack.SHARED_ENV, raising=False)
    calls: list[tuple[str, dict[str, Any]]] = []
    counter = {"n": 0}

    def fake_api(method: str, token: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        params = params or {}
        calls.append((method, params))
        if method == "auth.test":
            return {"ok": True, "user_id": _SELF_USER}
        if method == "chat.postMessage":
            counter["n"] += 1
            ts = f"1781739000.{counter['n']:06d}"
            return {
                "ok": True,
                "ts": ts,
                "channel": _CHANNEL_ID,
                "message": {"thread_ts": params.get("thread_ts", "")},
            }
        raise AssertionError(f"unexpected api method {method}")

    monkeypatch.setattr(slack, "_api_call", fake_api)
    return calls


def test_send_seeds_thread_and_watches(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "question for the team"))
    posts = [(m, p) for m, p in threaded_api if m == "chat.postMessage"]
    assert len(posts) == 2
    root_params, seed_params = posts[0][1], posts[1][1]
    assert "thread_ts" not in root_params
    assert seed_params["text"] == slack._THREAD_SEED_TEXT
    assert seed_params["thread_ts"] == out["ts"]
    assert out["watching"] is True
    assert "seed_error" not in out
    key = (_CHANNEL_ID, out["ts"])
    assert key in slack._watches
    # The seed itself is already "seen": it must never come back as a reply.
    assert slack._watches[key].last_seen_ts > out["ts"]


def test_send_in_thread_watches_parent(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "reply", thread_ts=_PARENT_TS))
    # No seed for an in-thread reply: exactly one post.
    assert len([m for m, _ in threaded_api if m == "chat.postMessage"]) == 1
    assert out["watching"] is True
    assert (_CHANNEL_ID, _PARENT_TS) in slack._watches


def test_send_watch_false_registers_nothing(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "fire and forget", watch=False, seed_thread=False))
    assert out["watching"] is False
    assert slack._watches == {}


def test_send_without_delivery_channel_reports_not_watching(
    monkeypatch: pytest.MonkeyPatch,
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    monkeypatch.setattr(slack, "_watches", {})
    monkeypatch.setattr(slack, "_resolve_notify", lambda: None)
    out = asyncio.run(slack.send(_CHANNEL_ID, "hello", seed_thread=False))
    assert out["watching"] is False
    assert slack._watches == {}


def _poll(
    monkeypatch: pytest.MonkeyPatch,
    replies: list[dict[str, Any]],
) -> None:
    """Swap in a replies-serving api and run one poll pass."""

    def fake_api(method: str, token: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        if method == "auth.test":
            return {"ok": True, "user_id": _SELF_USER}
        assert method == "conversations.replies"
        return {"ok": True, "messages": replies}

    monkeypatch.setattr(slack, "_api_call", fake_api)
    asyncio.run(slack._poll_watches_once())


def test_poll_notifies_on_reply_from_someone_else(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "anyone know?"))
    root = out["ts"]
    last_seen = slack._watches[(_CHANNEL_ID, root)].last_seen_ts
    _poll(
        monkeypatch,
        [
            {"ts": root, "user": _SELF_USER, "text": "anyone know?"},
            {"ts": last_seen, "user": _SELF_USER, "text": "."},
            {"ts": "1781739999.000001", "user": "U0OTHER0000", "text": "yes -- use X"},
        ],
    )
    assert len(fresh_watch_state) == 1
    content, meta = fresh_watch_state[0]
    assert "yes -- use X" in content
    assert meta["slack_user"] == "U0OTHER0000"
    assert meta["slack_thread_ts"] == root
    # Delivered replies advance the cursor: a second identical poll is silent.
    _poll(
        monkeypatch,
        [{"ts": "1781739999.000001", "user": "U0OTHER0000", "text": "yes -- use X"}],
    )
    assert len(fresh_watch_state) == 1


def test_poll_ignores_own_messages(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "note to self"))
    _poll(
        monkeypatch,
        [{"ts": "1781739999.000002", "user": _SELF_USER, "text": "my own follow-up"}],
    )
    assert fresh_watch_state == []
    # Own follow-ups still advance the cursor.
    assert slack._watches[(_CHANNEL_ID, out["ts"])].last_seen_ts == "1781739999.000002"


def test_poll_drops_watch_on_error_with_notice(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "will break"))

    def broken_api(method: str, token: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        if method == "auth.test":
            return {"ok": True, "user_id": _SELF_USER}
        raise slack.SlackError("boom")

    monkeypatch.setattr(slack, "_api_call", broken_api)
    asyncio.run(slack._poll_watches_once())
    assert slack._watches == {}
    assert len(fresh_watch_state) == 1
    assert fresh_watch_state[0][1]["slack_event"] == "watch_dropped"


def test_poll_keeps_watch_on_transient_error(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "rate-limited"))

    def limited_api(
        method: str, token: str, params: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        if method == "auth.test":
            return {"ok": True, "user_id": _SELF_USER}
        raise slack.SlackTransientError("Slack API HTTP 429 for conversations.replies")

    monkeypatch.setattr(slack, "_api_call", limited_api)
    asyncio.run(slack._poll_watches_once())
    # The watch survives a 429 and nothing spurious is delivered.
    assert (_CHANNEL_ID, out["ts"]) in slack._watches
    assert fresh_watch_state == []


def test_send_skips_seed_in_dms(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    out = asyncio.run(slack.send("D0123456789", "hey"))
    assert len([m for m, _ in threaded_api if m == "chat.postMessage"]) == 1
    assert out["watching"] is True


def test_login_and_logout_reset_identity_and_watches(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(slack, "_TOKEN_FILE", tmp_path / "token")
    asyncio.run(slack.send(_CHANNEL_ID, "before switch"))
    assert slack._watches
    monkeypatch.setattr(slack, "_self_user_id", "U0STALE0000")
    slack.login("xoxp-new-identity")
    assert slack._self_user_id is None
    slack.logout()
    assert slack._self_user_id is None
    assert slack._watches == {}


def test_unwatch_and_watches_frame(
    fresh_watch_state: list[tuple[str, dict[str, str]]],
    threaded_api: list[tuple[str, dict[str, Any]]],
) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "watched"))
    frame = slack.watches()
    assert frame.height == 1
    assert frame["channel_id"][0] == _CHANNEL_ID
    assert slack.unwatch(_CHANNEL_ID, out["ts"]) == {"removed": True}
    assert slack.unwatch(_CHANNEL_ID, out["ts"]) == {"removed": False}
    assert slack.watches().height == 0
