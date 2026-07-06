"""The streamable-HTTP transport's API-key gate and the CLI's bind policy.

The gate (`transport._gate`) is a plain ASGI wrapper, so it is driven directly
here with a stub inner app -- no server, no kernel. The bind policy
(`cli._http_bind_error`) is a pure predicate. Together they are the whole
contract for exposing the MCP endpoint beyond stdio: an unkeyed server never
listens beyond loopback/tailnet, and a keyed server 401s every request that
does not present the key.
"""

from __future__ import annotations

import asyncio

from ix_notebook_mcp import transport
from ix_notebook_mcp.cli import _http_bind_error

KEY = "sekret-0123"


def _run(
    app: transport._App,
    path: str = "/mcp",
    method: str = "POST",
    headers: list[tuple[bytes, bytes]] | None = None,
) -> tuple[object, bytes]:
    """Drive one request through an ASGI app; return (status, body)."""
    sent: list[dict[str, object]] = []

    async def receive() -> dict[str, object]:
        return {"type": "http.request", "body": b"", "more_body": False}

    async def send(message: transport._Message) -> None:
        sent.append(dict(message))

    scope: dict[str, object] = {"type": "http", "path": path, "method": method, "headers": headers or []}
    asyncio.run(app(scope, receive, send))
    status = next((m["status"] for m in sent if m["type"] == "http.response.start"), None)
    body = b""
    for m in sent:
        chunk = m.get("body")
        if m["type"] == "http.response.body" and isinstance(chunk, bytes):
            body += chunk
    return status, body


def _inner_recorder() -> tuple[transport._App, list[transport._Scope]]:
    hits: list[transport._Scope] = []

    async def inner(scope: transport._Scope, receive: transport._Receive, send: transport._Send) -> None:
        hits.append(scope)
        await transport._plain_response(send, 200, b"inner")

    return inner, hits


def test_no_key_configured_is_transparent() -> None:
    inner, hits = _inner_recorder()
    status, body = _run(transport._gate(inner, None))
    assert (status, body) == (200, b"inner")
    assert len(hits) == 1


def test_missing_key_is_401_before_inner() -> None:
    inner, hits = _inner_recorder()
    status, body = _run(transport._gate(inner, KEY))
    assert status == 401
    assert b"API key" in body
    assert hits == []


def test_wrong_key_is_401() -> None:
    inner, hits = _inner_recorder()
    gated = transport._gate(inner, KEY)
    status, _ = _run(gated, headers=[(b"x-api-key", b"wrong")])
    assert status == 401
    # A same-length wrong key must not pass either (constant-time compare, not length).
    status, _ = _run(gated, headers=[(b"x-api-key", KEY[:-1].encode() + b"X")])
    assert status == 401
    assert hits == []


def test_x_api_key_header_passes() -> None:
    inner, hits = _inner_recorder()
    status, body = _run(transport._gate(inner, KEY), headers=[(b"x-api-key", KEY.encode())])
    assert (status, body) == (200, b"inner")
    assert len(hits) == 1


def test_bearer_authorization_passes_and_x_api_key_wins() -> None:
    inner, hits = _inner_recorder()
    gated = transport._gate(inner, KEY)
    status, _ = _run(gated, headers=[(b"authorization", b"Bearer " + KEY.encode())])
    assert status == 200
    # X-Api-Key is authoritative when both are present, whatever the order.
    status, _ = _run(
        gated,
        headers=[(b"authorization", b"Bearer " + KEY.encode()), (b"x-api-key", b"wrong")],
    )
    assert status == 401
    assert len(hits) == 1


def test_health_is_open_even_when_keyed() -> None:
    inner, hits = _inner_recorder()
    status, body = _run(transport._gate(inner, KEY), path="/health", method="GET")
    assert (status, body) == (200, b"ok")
    assert hits == []
    # But only GET/HEAD: a POST to /health is not a probe and stays gated.
    status, _ = _run(transport._gate(inner, KEY), path="/health", method="POST")
    assert status == 401


def test_lifespan_scope_passes_through() -> None:
    seen: list[object] = []

    async def inner(scope: transport._Scope, receive: transport._Receive, send: transport._Send) -> None:
        seen.append(scope["type"])

    async def receive() -> dict[str, object]:
        return {"type": "lifespan.startup"}

    async def send(message: transport._Message) -> None:
        pass

    asyncio.run(transport._gate(inner, KEY)({"type": "lifespan"}, receive, send))
    assert seen == ["lifespan"]


def test_bind_policy_keyless_allows_only_loopback_and_tailnet() -> None:
    assert _http_bind_error("127.0.0.1", None) is None
    assert _http_bind_error("::1", None) is None
    assert _http_bind_error("localhost", None) is None
    assert _http_bind_error("100.69.184.31", None) is None  # tailnet CGNAT
    assert _http_bind_error("0.0.0.0", None) is not None  # noqa: S104 -- asserting the wildcard is refused
    assert _http_bind_error("::", None) is not None
    assert _http_bind_error("192.168.1.10", None) is not None  # LAN
    assert _http_bind_error("15.204.105.165", None) is not None  # public
    assert _http_bind_error("example.ix.dev", None) is not None  # unclassifiable name


def test_bind_policy_with_key_allows_everything() -> None:
    for host in ("127.0.0.1", "0.0.0.0", "::", "15.204.105.165", "example.ix.dev"):  # noqa: S104 -- policy table includes the wildcard
        assert _http_bind_error(host, KEY) is None
