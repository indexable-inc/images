"""Regression tests for ENG-2485: interactive OAuth (browser + PKCE) in mcp_client.

Everything runs against localhost only:

1. FileTokenStorage round-trips tokens and client info as private JSON
   (dir 0700, file 0600), keyed by a canonicalized server URL, and shrugs
   off corrupt files.
2. The loopback redirect listener serves /callback exactly once, validates
   nothing it doesn't have to, never reflects request data, and propagates
   authorization-server errors and timeouts.
3. The default redirect handler opens a browser when a display exists and
   prints the URL (instead of hanging) when headless.
4. End to end: a stub OAuth authorization server + bearer-gated streamable
   HTTP MCP server (both in-process). First connect() runs discovery, dynamic
   client registration, PKCE authorization and token exchange, then works;
   the second connect() reuses the cached token with no browser; an expired
   access token is refreshed silently; token= bypasses OAuth entirely.
"""

from __future__ import annotations

import asyncio
import json
import os
import secrets
import socket
import stat
import sys
from hashlib import sha256
from pathlib import Path

import httpx
import pytest
import pytest_asyncio

SRC = Path(__file__).parent / "src" / "mcp_client"
sys.path.insert(0, str(SRC))

import mcp_client  # noqa: E402
from mcp_client import _oauth  # noqa: E402

pytestmark = pytest.mark.asyncio


# ---------------------------------------------------------------------------
# 1. FileTokenStorage
# ---------------------------------------------------------------------------


async def test_storage_roundtrip_and_permissions(tmp_path):
    from mcp.shared.auth import OAuthClientInformationFull, OAuthToken

    base = tmp_path / "state" / "oauth"
    store = _oauth.FileTokenStorage("https://example.com/mcp", base_dir=base)
    assert await store.get_tokens() is None
    assert await store.get_client_info() is None

    tokens = OAuthToken(access_token="at-1", refresh_token="rt-1", expires_in=3600)
    await store.set_tokens(tokens)
    info = OAuthClientInformationFull(
        client_id="cid-1",
        redirect_uris=["http://127.0.0.1:50000/callback"],
        token_endpoint_auth_method="none",
    )
    await store.set_client_info(info)

    got_tokens = await store.get_tokens()
    assert got_tokens is not None
    assert got_tokens.access_token == "at-1"
    assert got_tokens.refresh_token == "rt-1"
    got_info = await store.get_client_info()
    assert got_info is not None
    assert got_info.client_id == "cid-1"

    # Private on disk: dir 0700, file 0600, and both fields in one file.
    assert stat.S_IMODE(store.path.parent.stat().st_mode) == 0o700
    assert stat.S_IMODE(store.path.stat().st_mode) == 0o600
    data = json.loads(store.path.read_text())
    assert data["tokens"]["access_token"] == "at-1"
    assert data["client_info"]["client_id"] == "cid-1"
    assert data["server_url"] == "https://example.com/mcp"

    # A fresh instance for the same server sees the same cache.
    again = _oauth.FileTokenStorage("https://example.com/mcp", base_dir=base)
    assert (await again.get_tokens()).access_token == "at-1"

    # clear() forgets the grant.
    store.clear()
    assert await store.get_tokens() is None
    store.clear()  # idempotent


async def test_storage_corrupt_file_returns_none(tmp_path):
    store = _oauth.FileTokenStorage("https://example.com/mcp", base_dir=tmp_path)
    store.path.parent.mkdir(parents=True, exist_ok=True)
    store.path.write_text("{not json")
    assert await store.get_tokens() is None
    assert await store.get_client_info() is None


async def test_storage_key_canonicalization(tmp_path):
    same = [
        "https://Example.COM/mcp",
        "https://example.com/mcp/",
        "https://example.com:443/mcp",
    ]
    paths = {_oauth.token_path(u, tmp_path) for u in same}
    assert len(paths) == 1
    assert _oauth.token_path("https://example.com/other", tmp_path) not in paths
    # The filename is a pure hash: no URL material leaks into the path.
    name = paths.pop().name
    assert name == sha256(b"https://example.com/mcp").hexdigest() + ".json"


async def test_default_token_dir_honors_xdg(monkeypatch, tmp_path):
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    assert _oauth.default_token_dir() == tmp_path / "ix-mcp" / "oauth"
    monkeypatch.delenv("XDG_STATE_HOME")
    assert _oauth.default_token_dir() == Path.home() / ".local" / "state" / "ix-mcp" / "oauth"


# ---------------------------------------------------------------------------
# 2. Loopback listener
# ---------------------------------------------------------------------------


async def test_loopback_delivers_code_and_state_once():
    listener = _oauth._LoopbackListener()
    uri = await listener.start()
    try:
        assert uri == f"http://127.0.0.1:{listener.port}/callback"
        async with httpx.AsyncClient() as client:
            # Wrong path: 404, does not consume the one-shot future.
            r = await client.get(f"http://127.0.0.1:{listener.port}/nope?code=x")
            assert r.status_code == 404

            r = await client.get(uri, params={"code": "abc", "state": "xyz"})
            assert r.status_code == 200
            assert "close this tab" in r.text.lower()
            # Static page: query data is never reflected.
            assert "abc" not in r.text and "xyz" not in r.text

            # A duplicate hit before the result is consumed is rejected.
            r = await client.get(uri, params={"code": "evil", "state": "evil"})
            assert r.status_code == 404

            code, state = await listener.wait(timeout=5)
            assert (code, state) == ("abc", "xyz")

            # Consuming the result re-arms the listener for a later flow
            # (expired grant / scope step-up on the same connection).
            r = await client.get(uri, params={"code": "abc2", "state": "xyz2"})
            assert r.status_code == 200
            assert await listener.wait(timeout=5) == ("abc2", "xyz2")
    finally:
        await listener.close()


async def test_loopback_error_redirect_raises():
    listener = _oauth._LoopbackListener()
    uri = await listener.start()
    try:
        async with httpx.AsyncClient() as client:
            r = await client.get(
                uri, params={"error": "access_denied", "error_description": "nope"}
            )
            assert r.status_code == 200
        with pytest.raises(_oauth.OAuthCallbackError, match="access_denied"):
            await listener.wait(timeout=5)
    finally:
        await listener.close()


async def test_loopback_timeout():
    listener = _oauth._LoopbackListener()
    await listener.start()
    try:
        with pytest.raises(_oauth.OAuthCallbackTimeout):
            await listener.wait(timeout=0.05)
    finally:
        await listener.close()


async def test_loopback_falls_back_when_preferred_port_busy():
    url = "https://busy.example.com/mcp"
    preferred = _oauth._preferred_port(url)
    blocker = socket.socket()
    blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    blocker.bind(("127.0.0.1", preferred))
    blocker.listen(1)
    listener = _oauth._LoopbackListener()
    try:
        await listener.start(preferred)
        assert listener.port != preferred
    finally:
        await listener.close()
        blocker.close()


async def test_preferred_port_is_stable_and_in_range():
    a = _oauth._preferred_port("https://example.com/mcp")
    b = _oauth._preferred_port("https://example.com/mcp/")
    assert a == b
    assert 49152 <= a <= 65535


# ---------------------------------------------------------------------------
# 3. Redirect handler (browser vs headless)
# ---------------------------------------------------------------------------


async def test_redirect_handler_headless_prints_url(monkeypatch, capsys):
    monkeypatch.delenv("DISPLAY", raising=False)
    monkeypatch.delenv("WAYLAND_DISPLAY", raising=False)
    monkeypatch.setattr(_oauth.sys, "platform", "linux")
    calls = []
    monkeypatch.setattr(_oauth.webbrowser, "open", lambda u: calls.append(u) or True)
    await _oauth._default_redirect_handler("https://auth.example/authorize?x=1")
    assert calls == []  # no browser attempt without a display
    err = capsys.readouterr().err
    assert "https://auth.example/authorize?x=1" in err
    assert "OAuth" in err


async def test_redirect_handler_opens_browser_with_display(monkeypatch, capsys):
    monkeypatch.setenv("DISPLAY", ":0")
    calls = []
    monkeypatch.setattr(_oauth.webbrowser, "open", lambda u: calls.append(u) or True)
    await _oauth._default_redirect_handler("https://auth.example/authorize")
    assert calls == ["https://auth.example/authorize"]
    assert capsys.readouterr().err == ""


async def test_redirect_handler_falls_back_to_print_when_browser_fails(
    monkeypatch, capsys
):
    monkeypatch.setenv("DISPLAY", ":0")

    def boom(_u):
        raise RuntimeError("no browser")

    monkeypatch.setattr(_oauth.webbrowser, "open", boom)
    await _oauth._default_redirect_handler("https://auth.example/authorize")
    assert "https://auth.example/authorize" in capsys.readouterr().err


# ---------------------------------------------------------------------------
# 4. End to end against a stub OAuth + MCP server
# ---------------------------------------------------------------------------


class StubOAuthMCPServer:
    """An in-process streamable-HTTP MCP server gated by a stub OAuth server.

    The MCP endpoint 401s (with a WWW-Authenticate pointing at RFC 9728
    metadata) until a known bearer token arrives. The OAuth half implements
    metadata discovery, RFC 7591 registration, /authorize (redirecting
    straight back to the client's loopback redirect_uri, standing in for the
    human consent step) and /token with full PKCE S256 verification.
    """

    def __init__(self) -> None:
        self.valid_tokens: set[str] = set()
        self.static_tokens: set[str] = {"static-secret"}
        self.codes: dict[str, dict] = {}
        self.registrations: list[dict] = []
        self.authorize_hits: list[dict] = []
        self.token_grants: list[str] = []
        self.refresh_tokens: dict[str, str] = {}
        self.port: int | None = None
        self._server = None

    @property
    def base(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def mcp_url(self) -> str:
        return f"{self.base}/mcp"

    async def start(self) -> None:
        import uvicorn
        from mcp.server.fastmcp import FastMCP
        from starlette.applications import Starlette
        from starlette.middleware.base import BaseHTTPMiddleware
        from starlette.requests import Request
        from starlette.responses import JSONResponse, RedirectResponse
        from starlette.routing import Mount, Route

        fast = FastMCP("stub", stateless_http=True, json_response=True)

        @fast.tool()
        def ping(text: str) -> str:
            """Echo the text back."""
            return f"pong: {text}"

        mcp_app = fast.streamable_http_app()
        stub = self

        async def prm(request: Request):
            return JSONResponse(
                {
                    "resource": stub.mcp_url,
                    "authorization_servers": [stub.base],
                }
            )

        async def asm(request: Request):
            return JSONResponse(
                {
                    "issuer": stub.base,
                    "authorization_endpoint": f"{stub.base}/authorize",
                    "token_endpoint": f"{stub.base}/token",
                    "registration_endpoint": f"{stub.base}/register",
                    "code_challenge_methods_supported": ["S256"],
                }
            )

        async def register(request: Request):
            body = await request.json()
            stub.registrations.append(body)
            return JSONResponse(
                {**body, "client_id": f"dcr-{len(stub.registrations)}"},
                status_code=201,
            )

        async def authorize(request: Request):
            q = dict(request.query_params)
            stub.authorize_hits.append(q)
            redirect_uri = q["redirect_uri"]
            if not redirect_uri.startswith("http://127.0.0.1:"):
                return JSONResponse({"error": "invalid_redirect_uri"}, status_code=400)
            if q.get("code_challenge_method") != "S256" or not q.get("code_challenge"):
                return JSONResponse({"error": "invalid_request"}, status_code=400)
            code = secrets.token_urlsafe(16)
            stub.codes[code] = q
            return RedirectResponse(
                f"{redirect_uri}?code={code}&state={q['state']}", status_code=302
            )

        async def token(request: Request):
            import base64
            from hashlib import sha256 as _sha256

            form = dict((await request.form()).items())
            grant = form.get("grant_type", "")
            stub.token_grants.append(grant)
            if grant == "authorization_code":
                issued = stub.codes.pop(form.get("code", ""), None)
                if issued is None:
                    return JSONResponse({"error": "invalid_grant"}, status_code=400)
                if form.get("redirect_uri") != issued["redirect_uri"]:
                    return JSONResponse({"error": "invalid_grant"}, status_code=400)
                if form.get("client_id") != issued["client_id"]:
                    return JSONResponse({"error": "invalid_client"}, status_code=400)
                verifier = form.get("code_verifier", "")
                challenge = (
                    base64.urlsafe_b64encode(_sha256(verifier.encode()).digest())
                    .decode()
                    .rstrip("=")
                )
                if challenge != issued["code_challenge"]:
                    return JSONResponse({"error": "invalid_grant"}, status_code=400)
            elif grant == "refresh_token":
                rt = form.get("refresh_token", "")
                if rt not in stub.refresh_tokens.values():
                    return JSONResponse({"error": "invalid_grant"}, status_code=400)
            else:
                return JSONResponse(
                    {"error": "unsupported_grant_type"}, status_code=400
                )
            access = f"at-{secrets.token_urlsafe(8)}"
            refresh = f"rt-{secrets.token_urlsafe(8)}"
            stub.valid_tokens.add(access)
            stub.refresh_tokens[access] = refresh
            return JSONResponse(
                {
                    "access_token": access,
                    "token_type": "Bearer",
                    "expires_in": 3600,
                    "refresh_token": refresh,
                }
            )

        class BearerGate(BaseHTTPMiddleware):
            async def dispatch(self, request, call_next):
                if request.url.path.startswith("/mcp"):
                    auth = request.headers.get("authorization", "")
                    tok = auth.removeprefix("Bearer ").strip()
                    if tok not in stub.valid_tokens | stub.static_tokens:
                        return JSONResponse(
                            {"error": "unauthorized"},
                            status_code=401,
                            headers={
                                "WWW-Authenticate": (
                                    "Bearer resource_metadata="
                                    f'"{stub.base}/.well-known/oauth-protected-resource/mcp"'
                                )
                            },
                        )
                return await call_next(request)

        app = Starlette(
            routes=[
                Route("/.well-known/oauth-protected-resource/mcp", prm),
                Route("/.well-known/oauth-protected-resource", prm),
                Route("/.well-known/oauth-authorization-server", asm),
                Route("/register", register, methods=["POST"]),
                Route("/authorize", authorize),
                Route("/token", token, methods=["POST"]),
                Mount("/", app=mcp_app),
            ],
            # Run the mounted MCP app's lifespan (it starts the session manager).
            lifespan=lambda _app: mcp_app.router.lifespan_context(mcp_app),
        )
        app.add_middleware(BearerGate)

        config = uvicorn.Config(app, host="127.0.0.1", port=0, log_level="error")
        self._server = uvicorn.Server(config)
        task = asyncio.create_task(self._server.serve())
        while not self._server.started:
            if task.done():
                task.result()
            await asyncio.sleep(0.01)
        self.port = self._server.servers[0].sockets[0].getsockname()[1]

    async def stop(self) -> None:
        if self._server is not None:
            self._server.should_exit = True
            await asyncio.sleep(0.05)


@pytest_asyncio.fixture
async def stub_server():
    srv = StubOAuthMCPServer()
    await srv.start()
    try:
        yield srv
    finally:
        await srv.stop()


@pytest.fixture
def oauth_env(monkeypatch, tmp_path):
    """Isolate the token cache and replace the browser with a local GET."""
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path))
    redirects: list[str] = []

    async def fake_redirect(url: str) -> None:
        redirects.append(url)

        def follow():
            with httpx.Client(follow_redirects=True, timeout=10) as client:
                client.get(url)

        await asyncio.to_thread(follow)

    monkeypatch.setattr(_oauth, "_default_redirect_handler", fake_redirect)
    return redirects


async def test_full_oauth_flow_then_cached_then_refresh(stub_server, oauth_env, tmp_path):
    url = stub_server.mcp_url

    # --- first connect: full interactive flow ---
    srv = await mcp_client.connect(url, timeout=10, oauth_timeout=20)
    try:
        assert len(oauth_env) == 1, "browser consent should run exactly once"
        assert "code_challenge=" in oauth_env[0] and "state=" in oauth_env[0]
        assert srv.tools.height == 1
        out = await srv.call("ping", text="hi")
        assert out.text == "pong: hi"
    finally:
        await srv.close()

    # DCR happened once; PKCE was verified server-side (else /token 400s).
    assert len(stub_server.registrations) == 1
    assert stub_server.token_grants == ["authorization_code"]

    # Tokens cached privately on disk.
    path = _oauth.token_path(url)
    assert path.exists()
    assert str(path).startswith(str(tmp_path))
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    cached = json.loads(path.read_text())
    assert cached["tokens"]["access_token"] in stub_server.valid_tokens
    assert cached["client_info"]["client_id"] == "dcr-1"

    # --- second connect: cached token, no browser, no new grant ---
    srv2 = await mcp_client.connect(url, timeout=10, oauth_timeout=20)
    try:
        assert len(oauth_env) == 1, "second connect must not open a browser"
        assert stub_server.token_grants == ["authorization_code"]
        out = await srv2.call("ping", text="again")
        assert out.text == "pong: again"
    finally:
        await srv2.close()

    # --- third connect with an expired access token: silent refresh ---
    import time

    cached = json.loads(path.read_text())
    cached["tokens"]["access_token"] = "at-expired"
    cached["expires_at"] = time.time() - 10
    path.write_text(json.dumps(cached))
    srv3 = await mcp_client.connect(url, timeout=10, oauth_timeout=20)
    try:
        assert len(oauth_env) == 1, "refresh must not open a browser"
        assert stub_server.token_grants == ["authorization_code", "refresh_token"]
        out = await srv3.call("ping", text="fresh")
        assert out.text == "pong: fresh"
    finally:
        await srv3.close()


async def test_static_token_bypasses_oauth(stub_server, oauth_env):
    srv = await mcp_client.connect(stub_server.mcp_url, token="static-secret", timeout=10)
    try:
        assert oauth_env == []
        assert stub_server.registrations == []
        assert stub_server.authorize_hits == []
        out = await srv.call("ping", text="direct")
        assert out.text == "pong: direct"
    finally:
        await srv.close()
    assert not _oauth.token_path(stub_server.mcp_url).exists()


async def test_oauth_false_disables_flow(stub_server, oauth_env):
    with pytest.raises(Exception):
        await mcp_client.connect(stub_server.mcp_url, oauth=False, timeout=5)
    assert oauth_env == []
    assert stub_server.authorize_hits == []


async def test_configured_client_id_skips_registration(stub_server, oauth_env):
    srv = await mcp_client.connect(
        stub_server.mcp_url,
        client_id="preconfigured-id",
        timeout=10,
        oauth_timeout=20,
    )
    try:
        assert len(oauth_env) == 1
        assert stub_server.registrations == [], "no DCR with a configured client_id"
        assert stub_server.authorize_hits[0]["client_id"] == "preconfigured-id"
        out = await srv.call("ping", text="cid")
        assert out.text == "pong: cid"
    finally:
        await srv.close()
