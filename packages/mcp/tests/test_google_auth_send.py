"""Network-free tests for `google_auth.send` (issue #2523).

These never reach Gmail: they stub the one network primitive (`gmail()`) with a
fake googleapiclient Resource and check that `send` assembles the MIME message
correctly, threads replies (threadId + In-Reply-To/References), and returns the
body read back from the message the API stored, not an echo of the arguments.
"""

from __future__ import annotations

import asyncio
import base64
import email
import sys
from pathlib import Path
from typing import Any

import pytest

# Prefer the bundled module (nix check); fall back to the source tree (dev run).
GOOGLE_AUTH_SRC = Path(__file__).resolve().parents[1] / "src" / "google_auth"
if GOOGLE_AUTH_SRC.is_dir() and str(GOOGLE_AUTH_SRC) not in sys.path:
    sys.path.insert(0, str(GOOGLE_AUTH_SRC))

import google_auth

_ORIGINAL_ID = "18f0deadbeef0001"
_ORIGINAL_THREAD = "18f0deadbeef0000"
_ORIGINAL_MESSAGE_ID = "<orig@mail.example.com>"
_SENT_ID = "18f0deadbeef0002"


def _b64(text: str) -> str:
    return base64.urlsafe_b64encode(text.encode()).decode("ascii")


class _Request:
    """A googleapiclient HttpRequest stand-in: `.execute()` yields the response."""

    def __init__(self, response: dict[str, Any]) -> None:
        self._response = response

    def execute(self) -> dict[str, Any]:
        return self._response


class FakeGmail:
    """The slice of the Gmail Resource `send` uses, with call capture.

    `send` stores the raw MIME; a later `get` of the sent id answers with a
    payload rebuilt from what was actually "stored", so the readback assertion
    is real (the returned body comes from the API, not the arguments).
    """

    def __init__(self, *, original_headers: list[dict[str, str]] | None = None) -> None:
        self.sent_bodies: list[dict[str, Any]] = []
        self.get_calls: list[dict[str, Any]] = []
        self._original_headers = original_headers or [
            {"name": "Message-ID", "value": _ORIGINAL_MESSAGE_ID}
        ]

    # The chained Resource shape: users().messages().<verb>(...)
    def users(self) -> FakeGmail:
        return self

    def messages(self) -> FakeGmail:
        return self

    def send(self, userId: str, body: dict[str, Any]) -> _Request:
        assert userId == "me"
        self.sent_bodies.append(body)
        return _Request({"id": _SENT_ID, "threadId": body.get("threadId", _SENT_ID)})

    def get(self, userId: str, id: str, **kwargs: str | list[str]) -> _Request:
        assert userId == "me"
        self.get_calls.append({"id": id, **kwargs})
        if id == _ORIGINAL_ID:
            return _Request(
                {
                    "id": _ORIGINAL_ID,
                    "threadId": _ORIGINAL_THREAD,
                    "payload": {"headers": self._original_headers},
                }
            )
        # The sent message read back: body rebuilt from the stored raw MIME.
        assert id == _SENT_ID, f"unexpected get({id})"
        raw = self.sent_bodies[-1]["raw"]
        stored = email.message_from_bytes(base64.urlsafe_b64decode(raw))
        text = stored.get_payload(decode=True).decode()
        return _Request(
            {
                "id": _SENT_ID,
                "threadId": self.sent_bodies[-1].get("threadId", _SENT_ID),
                "payload": {"mimeType": "text/plain", "body": {"data": _b64(text)}},
            }
        )


@pytest.fixture
def fake_gmail(monkeypatch: pytest.MonkeyPatch) -> FakeGmail:
    monkeypatch.delenv(google_auth.SHARED_ENV, raising=False)
    fake = FakeGmail()
    monkeypatch.setattr(google_auth, "gmail", lambda: fake)
    return fake


def _sent_mime(fake: FakeGmail) -> email.message.Message:
    return email.message_from_bytes(base64.urlsafe_b64decode(fake.sent_bodies[-1]["raw"]))


def test_send_assembles_mime(fake_gmail: FakeGmail) -> None:
    out = asyncio.run(
        google_auth.send("to@example.com", "hello", "the body\n", cc="cc@example.com")
    )
    mime = _sent_mime(fake_gmail)
    assert mime["To"] == "to@example.com"
    assert mime["Subject"] == "hello"
    assert mime["Cc"] == "cc@example.com"
    assert mime.get_payload(decode=True).decode() == "the body\n"
    # A fresh message carries no reply headers and no threadId.
    assert mime["In-Reply-To"] is None
    assert mime["References"] is None
    assert "threadId" not in fake_gmail.sent_bodies[-1]
    assert out["id"] == _SENT_ID


def test_send_omits_cc_by_default(fake_gmail: FakeGmail) -> None:
    asyncio.run(google_auth.send("to@example.com", "s", "b"))
    assert _sent_mime(fake_gmail)["Cc"] is None


def test_reply_threads(fake_gmail: FakeGmail) -> None:
    out = asyncio.run(
        google_auth.send(
            "to@example.com", "Re: hello", "reply body", reply_to_message_id=_ORIGINAL_ID
        )
    )
    # The original was fetched as metadata with the reply headers requested.
    lookup = fake_gmail.get_calls[0]
    assert lookup["id"] == _ORIGINAL_ID
    assert lookup["format"] == "metadata"
    assert set(lookup["metadataHeaders"]) == {"Message-ID", "References"}

    mime = _sent_mime(fake_gmail)
    assert mime["In-Reply-To"] == _ORIGINAL_MESSAGE_ID
    assert mime["References"] == _ORIGINAL_MESSAGE_ID
    assert fake_gmail.sent_bodies[-1]["threadId"] == _ORIGINAL_THREAD
    assert out["thread_id"] == _ORIGINAL_THREAD


def test_reply_extends_references_chain(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(google_auth.SHARED_ENV, raising=False)
    fake = FakeGmail(
        original_headers=[
            {"name": "Message-ID", "value": _ORIGINAL_MESSAGE_ID},
            {"name": "References", "value": "<root@mail.example.com>"},
        ]
    )
    monkeypatch.setattr(google_auth, "gmail", lambda: fake)
    asyncio.run(google_auth.send("to@example.com", "Re: s", "b", reply_to_message_id=_ORIGINAL_ID))
    assert _sent_mime(fake)["References"] == f"<root@mail.example.com> {_ORIGINAL_MESSAGE_ID}"


def test_returns_body_read_back(fake_gmail: FakeGmail) -> None:
    out = asyncio.run(google_auth.send("to@example.com", "s", "verify me"))
    # The body comes from the stored message via get(), not from the argument:
    # the second get call targets the sent id.
    assert fake_gmail.get_calls[-1]["id"] == _SENT_ID
    assert fake_gmail.get_calls[-1]["format"] == "full"
    assert out["body"].rstrip("\n") == "verify me"


def test_payload_text_recurses_multipart() -> None:
    payload = {
        "mimeType": "multipart/alternative",
        "parts": [
            {"mimeType": "text/html", "body": {"data": _b64("<b>nope</b>")}},
            {"mimeType": "text/plain", "body": {"data": _b64("plain wins")}},
        ],
    }
    assert google_auth._payload_text(payload) == "plain wins"


def test_shared_room_refuses(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(google_auth.SHARED_ENV, "1")
    with pytest.raises(google_auth.GoogleAuthError, match="shared"):
        asyncio.run(google_auth.send("to@example.com", "s", "b"))
