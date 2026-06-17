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

# Public callables = everything exported except the error class.
_PUBLIC_FUNCS = [getattr(slack, name) for name in slack.__all__ if name != "SlackError"]

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
    out = asyncio.run(slack.send(_CHANNEL_ID, "hello"))
    method, params = stub_slack[-1]
    assert method == "chat.postMessage"
    assert "thread_ts" not in params
    assert out["thread_ts"] == ""
    assert out["ok"] is True


def test_send_in_thread_passes_thread_ts(stub_slack: list[tuple[str, dict[str, Any]]]) -> None:
    out = asyncio.run(slack.send(_CHANNEL_ID, "reply", thread_ts=_PARENT_TS))
    _, params = stub_slack[-1]
    assert params["thread_ts"] == _PARENT_TS
    assert "reply_broadcast" not in params
    assert out["thread_ts"] == _PARENT_TS


def test_send_reply_broadcast_sets_flag(stub_slack: list[tuple[str, dict[str, Any]]]) -> None:
    asyncio.run(slack.send(_CHANNEL_ID, "loud reply", thread_ts=_PARENT_TS, reply_broadcast=True))
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
