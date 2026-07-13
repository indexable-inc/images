"""Store credential handling against an authenticated Weave.

Weave deployments behind ``--proxy-identity`` (or with API tokens) reject
anonymous ``/api/facts`` writes with 401, which silently killed the whole
kernel-presence lane: the write-behind writer backed off and retried
forever, no kernel entity ever landed, and the board showed no kernel.

Contract pinned here:
- ``WEAVE_TOKEN`` rides every request against ``WEAVE_URL`` as ``X-Api-Key``
  (the same header the sibling ``weave`` client module sends); no token
  means no header, so open-loopback dev servers are untouched.
- The mailbox/data API is a different trust domain: never send the token.
- A 401/403 from Weave is permanent for the process: the writer logs one
  loud line and drops to the disabled mode instead of retrying forever.
- Any other failure keeps the existing retry-with-backoff behavior.
"""

from __future__ import annotations

from typing import TYPE_CHECKING
from urllib.error import HTTPError

import pytest

from ix_notebook_mcp import store

if TYPE_CHECKING:
    from pathlib import Path


def _capture(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, str, dict]]:
    requests: list[tuple[str, str, dict]] = []

    def fake_json(
        method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None
    ) -> object:
        requests.append((method, url, dict(headers or {})))
        if url.endswith("/api/blob"):
            return {"hash": "0" * 64}
        if url.endswith("/api/query"):
            return {"vars": [], "rows": [], "as_of": 0}
        return {"seq": 1, "id": "f1"}

    def fake_bytes(method: str, url: str, *, content: bytes | None = None, headers: dict | None = None) -> bytes:
        requests.append((method, url, dict(headers or {})))
        return b""

    monkeypatch.setattr(store, "_http_json", fake_json)
    monkeypatch.setattr(store, "_http_bytes", fake_bytes)
    return requests


def test_weave_token_rides_every_weave_request(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("WEAVE_TOKEN", "s3cret")
    requests = _capture(monkeypatch)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        assert conn.flush(timeout=5.0)
        conn.put_blob(b"payload")
        conn.get_blob("0" * 64)
        conn.query("?- latest(E, A, V).")
    finally:
        conn.close()

    weave_requests = [r for r in requests if r[1].startswith("http://weave.test")]
    assert weave_requests, "no weave traffic recorded"
    for method, url, headers in weave_requests:
        assert headers.get("X-Api-Key") == "s3cret", f"{method} {url} missing credential: {headers}"


def test_without_token_no_auth_header_is_sent(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.delenv("WEAVE_TOKEN", raising=False)
    requests = _capture(monkeypatch)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        assert conn.flush(timeout=5.0)
    finally:
        conn.close()

    assert requests
    for _, _, headers in requests:
        assert "X-Api-Key" not in headers


def _rejecting(monkeypatch: pytest.MonkeyPatch, code: int) -> list[str]:
    urls: list[str] = []

    def deny(
        method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None
    ) -> object:
        urls.append(url)
        raise HTTPError(url, code, "denied", None, None)  # type: ignore[arg-type]

    monkeypatch.setattr(store, "_http_json", deny)
    return urls


@pytest.mark.parametrize("code", [401, 403])
def test_auth_rejection_disables_writes_permanently(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], code: int
) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.delenv("WEAVE_TOKEN", raising=False)
    attempts = _rejecting(monkeypatch, code)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        assert conn.flush(timeout=10.0), "a permanent rejection must not wedge flush()"
        assert conn.disabled, "an auth rejection must disable the store"
        assert len(attempts) == 1, f"an auth rejection must not be retried: {attempts}"
        # Disabled means later durable writes are dropped without touching
        # the network, exactly like WEAVE_URL=off.
        before = len(attempts)
        store.start(conn, id="c1", name="probe", code="pass", started_at=0.0)
        assert conn.flush(timeout=5.0)
        assert len(attempts) == before
    finally:
        conn.close()

    err = capsys.readouterr().err
    assert "DISABLED" in err
    assert "WEAVE_TOKEN" in err


def test_transient_failures_still_retry(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    calls: list[str] = []

    def flaky(
        method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None
    ) -> object:
        calls.append(url)
        if len(calls) == 1:
            raise ConnectionError("weave not up yet")
        return {"seq": 1, "id": "f1"}

    monkeypatch.setattr(store, "_http_json", flaky)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        assert conn.flush(timeout=10.0)
        assert not conn.disabled
        assert len(calls) >= 2, "a transient failure must be retried"
    finally:
        conn.close()
