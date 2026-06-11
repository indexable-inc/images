"""Interactive OAuth 2.0 + PKCE support for ``mcp_client.connect``.

Many remote MCP servers (Todoist's ``https://ai.todoist.net/mcp``, the GitHub
remote MCP, ...) gate access behind an OAuth 2.0 authorization-code flow with
PKCE instead of a static bearer token. The bundled ``mcp`` SDK already ships
the protocol engine (:class:`mcp.client.auth.OAuthClientProvider` drives
discovery, dynamic client registration, PKCE, token exchange and refresh as an
``httpx`` auth flow); this module supplies the three environment-specific
pieces it needs and wires them into a transport context manager:

* :class:`FileTokenStorage` -- tokens and client registrations cached as
  private JSON files (dir ``0700``, files ``0600``) under
  ``$XDG_STATE_HOME/ix-mcp/oauth`` (default ``~/.local/state/ix-mcp/oauth``),
  keyed by a hash of the server URL, so a later ``connect`` reuses the grant
  without a browser.
* a loopback redirect listener on ``http://127.0.0.1:<port>/callback`` that
  catches the authorization code exactly once and tells the human to close
  the tab.
* a redirect handler that opens the system browser when a display is
  available and otherwise prints the authorization URL, so headless sessions
  get a copy-pasteable link instead of a hang.

Nothing here talks to the network on its own: the provider only starts the
flow when the server answers 401, so attaching it to a public server is free.
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import stat
import sys
import time
import webbrowser
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Callable
from urllib.parse import parse_qs, urlsplit

from mcp.client.auth import OAuthClientProvider
from mcp.shared.auth import OAuthClientInformationFull, OAuthClientMetadata, OAuthToken

__all__ = [
    "FileTokenStorage",
    "OAuthCallbackError",
    "OAuthCallbackTimeout",
    "default_token_dir",
    "oauth_transport",
    "token_path",
]

# Loopback ports are picked deterministically from the server URL inside the
# dynamic/private range (RFC 6335), so re-authorizing against the same server
# reuses the redirect URI that dynamic client registration stored. If the
# preferred port is taken we fall back to an ephemeral one.
_PORT_RANGE_START = 49152
_PORT_RANGE_SIZE = 16000

_CALLBACK_PATH = "/callback"

_SUCCESS_PAGE = (
    "<!doctype html><html><head><title>mcp_client</title></head>"
    "<body style='font-family: sans-serif; margin: 4em;'>"
    "<h2>Authorization complete.</h2>"
    "<p>You may close this tab and return to your session.</p>"
    "</body></html>"
)
_DENIED_PAGE = (
    "<!doctype html><html><head><title>mcp_client</title></head>"
    "<body style='font-family: sans-serif; margin: 4em;'>"
    "<h2>Authorization failed.</h2>"
    "<p>The authorization server reported an error. You may close this tab; "
    "see your session for details.</p>"
    "</body></html>"
)


class OAuthCallbackTimeout(RuntimeError):
    """Raised when no OAuth redirect arrives before the flow timeout."""


class OAuthCallbackError(RuntimeError):
    """Raised when the authorization server redirects back with an error."""


# --- token storage ------------------------------------------------------------


def default_token_dir() -> Path:
    """The directory OAuth state is cached under (honors ``XDG_STATE_HOME``)."""
    base = os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")
    return Path(base) / "ix-mcp" / "oauth"


def _canonical_url(server_url: str) -> str:
    """Normalize a server URL so trivially different spellings share a cache."""
    parts = urlsplit(server_url)
    netloc = parts.netloc.lower()
    if (parts.scheme.lower(), netloc.rpartition(":")[2]) in (
        ("http", "80"),
        ("https", "443"),
    ):
        netloc = netloc.rpartition(":")[0]
    return f"{parts.scheme.lower()}://{netloc}{parts.path.rstrip('/')}"


def token_path(server_url: str, base_dir: Path | None = None) -> Path:
    """Where tokens for ``server_url`` are cached (file may not exist yet)."""
    base = base_dir or default_token_dir()
    digest = hashlib.sha256(_canonical_url(server_url).encode()).hexdigest()
    return base / f"{digest}.json"


def _secure_mkdir(path: Path) -> None:
    """Create ``path`` (and parents) and harden every dir we own under the base.

    Mirrors the CWE-377 stance of ``ix_notebook_mcp.config.runtime_dir``: fail
    closed if the leaf is a symlink or owned by someone else, and strip
    group/other access.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    path.mkdir(mode=0o700, exist_ok=True)
    for p in (path.parent, path) if path.parent.name == "ix-mcp" else (path,):
        info = p.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise RuntimeError(f"oauth state dir {p} is not a real directory")
        if info.st_uid != os.getuid():
            raise RuntimeError(f"oauth state dir {p} is not owned by the current user")
        if info.st_mode & 0o077:
            p.chmod(0o700)


def _write_private(path: Path, payload: dict[str, Any]) -> None:
    """Atomically write ``payload`` as JSON readable only by the current user."""
    import secrets as _secrets

    tmp = path.with_name(f"{path.name}.tmp.{os.getpid()}.{_secrets.token_hex(4)}")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(fd, "w") as fh:
            json.dump(payload, fh, indent=2)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
    os.replace(tmp, path)


class FileTokenStorage:
    """A :class:`mcp.client.auth.TokenStorage` backed by one private JSON file.

    The file holds both the OAuth tokens and the (dynamically registered)
    client information for a single server URL, so a re-``connect`` can skip
    both registration and the browser as long as the grant is alive.
    """

    def __init__(self, server_url: str, base_dir: Path | None = None) -> None:
        self.server_url = server_url
        self.path = token_path(server_url, base_dir)

    # -- internal --

    def _load(self) -> dict[str, Any]:
        try:
            with self.path.open() as fh:
                data = json.load(fh)
        except (OSError, ValueError):
            return {}
        return data if isinstance(data, dict) else {}

    def _store(self, data: dict[str, Any]) -> None:
        _secure_mkdir(self.path.parent)
        data["server_url"] = self.server_url
        _write_private(self.path, data)

    def clear(self) -> None:
        """Forget the cached grant (next connect re-runs the browser flow)."""
        try:
            self.path.unlink()
        except FileNotFoundError:
            pass

    # -- TokenStorage protocol --

    async def get_tokens(self) -> OAuthToken | None:
        raw = self._load().get("tokens")
        if not raw:
            return None
        try:
            return OAuthToken.model_validate(raw)
        except Exception:
            return None

    async def set_tokens(self, tokens: OAuthToken) -> None:
        data = self._load()
        data["tokens"] = tokens.model_dump(mode="json", exclude_none=True)
        # `expires_in` is relative to the grant; persist the absolute deadline
        # so a later process can refresh a stale token instead of re-prompting.
        data["expires_at"] = (
            time.time() + tokens.expires_in if tokens.expires_in else None
        )
        self._store(data)

    def expires_at(self) -> float | None:
        """Absolute expiry (unix time) of the cached access token, if known."""
        value = self._load().get("expires_at")
        return float(value) if isinstance(value, (int, float)) else None

    async def get_client_info(self) -> OAuthClientInformationFull | None:
        raw = self._load().get("client_info")
        if not raw:
            return None
        try:
            return OAuthClientInformationFull.model_validate(raw)
        except Exception:
            return None

    async def set_client_info(self, client_info: OAuthClientInformationFull) -> None:
        data = self._load()
        data["client_info"] = client_info.model_dump(mode="json", exclude_none=True)
        self._store(data)


class _StaticClientStorage:
    """Wrap a storage so a caller-configured ``client_id`` wins over DCR.

    Tokens still round-trip through the underlying file storage; only the
    client information is pinned (and never persisted, so dropping the kwarg
    falls back cleanly to dynamic registration).
    """

    def __init__(
        self, inner: FileTokenStorage, client_info: OAuthClientInformationFull
    ) -> None:
        self._inner = inner
        self._client_info = client_info

    async def get_tokens(self) -> OAuthToken | None:
        return await self._inner.get_tokens()

    async def set_tokens(self, tokens: OAuthToken) -> None:
        await self._inner.set_tokens(tokens)

    async def get_client_info(self) -> OAuthClientInformationFull | None:
        return self._client_info

    async def set_client_info(self, client_info: OAuthClientInformationFull) -> None:
        return None


# --- loopback redirect listener -------------------------------------------------


class _LoopbackListener:
    """A tiny one-shot HTTP listener on 127.0.0.1 that catches the redirect.

    Serves ``GET /callback?code=...&state=...`` exactly once (later hits and
    other paths get a 404), answers with a static "close this tab" page (no
    request data is ever reflected, so nothing the authorization server or a
    local attacker appends can be smuggled into the response), and hands the
    ``(code, state)`` pair to :meth:`wait`.
    """

    host = "127.0.0.1"

    def __init__(self) -> None:
        self._server: asyncio.AbstractServer | None = None
        self._result: asyncio.Future | None = None
        self.port: int | None = None

    async def start(self, preferred_port: int | None = None) -> str:
        self._result = asyncio.get_running_loop().create_future()
        if preferred_port:
            try:
                self._server = await asyncio.start_server(
                    self._handle, self.host, preferred_port
                )
            except OSError:
                self._server = None
        if self._server is None:
            self._server = await asyncio.start_server(self._handle, self.host, 0)
        self.port = self._server.sockets[0].getsockname()[1]
        return f"http://{self.host}:{self.port}{_CALLBACK_PATH}"

    async def _handle(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        try:
            request_line = await asyncio.wait_for(reader.readline(), timeout=10)
            # Drain the headers; a redirect GET has no body we care about.
            while True:
                line = await asyncio.wait_for(reader.readline(), timeout=10)
                if line in (b"\r\n", b"\n", b""):
                    break
            parts = request_line.decode("latin-1", "replace").split()
            target = parts[1] if len(parts) >= 2 else ""
            path, _, query = target.partition("?")
            done = self._result is None or self._result.done()
            if parts[:1] != ["GET"] or path != _CALLBACK_PATH or done:
                await self._respond(writer, "404 Not Found", "not found")
                return
            params = parse_qs(query, keep_blank_values=True)
            error = (params.get("error") or [None])[0]
            if error is not None:
                desc = (params.get("error_description") or [""])[0]
                self._result.set_result(
                    OAuthCallbackError(
                        f"authorization failed: {error}" + (f" ({desc})" if desc else "")
                    )
                )
                await self._respond(writer, "200 OK", _DENIED_PAGE, html=True)
                return
            code = (params.get("code") or [None])[0]
            state = (params.get("state") or [None])[0]
            self._result.set_result((code, state))
            await self._respond(writer, "200 OK", _SUCCESS_PAGE, html=True)
        except Exception:
            pass
        finally:
            try:
                writer.close()
                await writer.wait_closed()
            except Exception:
                pass

    @staticmethod
    async def _respond(
        writer: asyncio.StreamWriter, status: str, body: str, *, html: bool = False
    ) -> None:
        payload = body.encode()
        ctype = "text/html; charset=utf-8" if html else "text/plain; charset=utf-8"
        writer.write(
            (
                f"HTTP/1.1 {status}\r\n"
                f"Content-Type: {ctype}\r\n"
                f"Content-Length: {len(payload)}\r\n"
                "Connection: close\r\n\r\n"
            ).encode()
            + payload
        )
        await writer.drain()

    async def wait(self, timeout: float) -> tuple[str | None, str | None]:
        """Block until the redirect lands; raise on error or timeout.

        Consuming a result re-arms the listener so a later flow on the same
        connection (expired grant, scope step-up) gets a fresh one-shot
        instead of replaying this result.
        """
        assert self._result is not None, "start() must be called first"
        result = self._result
        try:
            outcome = await asyncio.wait_for(asyncio.shield(result), timeout)
        except asyncio.TimeoutError:
            raise OAuthCallbackTimeout(
                f"no OAuth redirect received on 127.0.0.1:{self.port} "
                f"within {timeout:.0f}s"
            ) from None
        finally:
            if result.done() and self._result is result:
                self._result = asyncio.get_running_loop().create_future()
        if isinstance(outcome, BaseException):
            raise outcome
        return outcome

    async def close(self) -> None:
        if self._server is not None:
            self._server.close()
            try:
                await self._server.wait_closed()
            except Exception:
                pass
            self._server = None


def _preferred_port(server_url: str) -> int:
    digest = hashlib.sha256(_canonical_url(server_url).encode()).hexdigest()
    return _PORT_RANGE_START + int(digest, 16) % _PORT_RANGE_SIZE


# --- redirect handler -----------------------------------------------------------


def _has_display() -> bool:
    if sys.platform in ("darwin", "win32"):
        return True
    return bool(os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY"))


async def _default_redirect_handler(authorization_url: str) -> None:
    """Open the consent page in a browser; print the URL when that can't work."""
    opened = False
    if _has_display():
        try:
            opened = await asyncio.to_thread(webbrowser.open, authorization_url)
        except Exception:
            opened = False
    if not opened:
        print(
            "mcp_client: this server requires OAuth authorization.\n"
            "Open this URL in a browser to continue:\n\n"
            f"  {authorization_url}\n",
            file=sys.stderr,
            flush=True,
        )


class _RestoringOAuthProvider(OAuthClientProvider):
    """An :class:`OAuthClientProvider` that restores token expiry from disk.

    The SDK's ``_initialize`` loads cached tokens but leaves the in-memory
    expiry clock unset (the stored ``expires_in`` is relative to a grant that
    happened in some earlier process), so a stale access token would be sent,
    bounce with 401, and re-open the browser. Restoring the absolute deadline
    persisted by :class:`FileTokenStorage` lets the provider notice the
    expiry up front and refresh silently instead.
    """

    def __init__(self, *args: Any, file_storage: FileTokenStorage, **kwargs: Any):
        super().__init__(*args, **kwargs)
        self._file_storage = file_storage

    async def _initialize(self) -> None:
        await super()._initialize()
        if self.context.current_tokens is not None:
            expires_at = self._file_storage.expires_at()
            if expires_at is not None:
                self.context.token_expiry_time = expires_at


# --- the transport wrapper ------------------------------------------------------


def _static_client_info(
    client_id: str,
    client_secret: str | None,
    metadata: OAuthClientMetadata,
) -> OAuthClientInformationFull:
    return OAuthClientInformationFull(
        client_id=client_id,
        client_secret=client_secret,
        redirect_uris=metadata.redirect_uris,
        token_endpoint_auth_method=metadata.token_endpoint_auth_method,
        grant_types=metadata.grant_types,
        response_types=metadata.response_types,
        scope=metadata.scope,
    )


@asynccontextmanager
async def oauth_transport(
    make_transport: Callable[[OAuthClientProvider], Any],
    server_url: str,
    *,
    client_id: str | None = None,
    client_secret: str | None = None,
    scopes: str | list[str] | None = None,
    flow_timeout: float = 300.0,
    storage: FileTokenStorage | None = None,
    redirect_handler: Callable[..., Any] | None = None,
):
    """Run an MCP transport with interactive OAuth attached.

    ``make_transport`` is called with an :class:`OAuthClientProvider` (an
    ``httpx.Auth``) and must return the transport's async context manager;
    whatever it yields is passed through. The loopback listener lives for the
    whole connection so a mid-session re-authorization (expired grant, scope
    step-up) reuses the same redirect URI that was registered.
    """
    listener = _LoopbackListener()
    store = storage or FileTokenStorage(server_url)
    redirect = redirect_handler or _default_redirect_handler
    redirect_uri = await listener.start(_preferred_port(server_url))
    try:
        scope = " ".join(scopes) if isinstance(scopes, (list, tuple)) else scopes
        metadata = OAuthClientMetadata(
            client_name="ix mcp_client",
            redirect_uris=[redirect_uri],  # pydantic coerces to AnyUrl
            grant_types=["authorization_code", "refresh_token"],
            response_types=["code"],
            token_endpoint_auth_method=(
                "client_secret_post" if client_secret else "none"
            ),
            scope=scope,
        )
        if client_id:
            provider_storage: Any = _StaticClientStorage(
                store, _static_client_info(client_id, client_secret, metadata)
            )
        else:
            provider_storage = store

        async def _callback() -> tuple[str | None, str | None]:
            return await listener.wait(flow_timeout)

        provider = _RestoringOAuthProvider(
            server_url=server_url,
            client_metadata=metadata,
            storage=provider_storage,
            redirect_handler=redirect,
            callback_handler=_callback,
            timeout=flow_timeout,
            file_storage=store,
        )
        async with make_transport(provider) as streams:
            yield streams
    finally:
        await listener.close()
