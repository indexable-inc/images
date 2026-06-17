"""Beeper for the kernel: read accounts, chats, and messages across every network; search; send.

Beeper Desktop exposes a fully local HTTP API (default ``http://localhost:23373``)
that aggregates chats from WhatsApp, Telegram, Signal, iMessage, Instagram,
Messenger, Discord, Slack, X, LinkedIn, Google Messages, and more. This module
bundles a thin, polars-shaped wrapper so a session can ``import beeper`` with no
install step.

    import beeper

    beeper.login("<access token>")     # store your token (written mode 0600)
    beeper.status()                    # {"configured": True, "base_url": ..., "version": ...}
    beeper.logout()                    # remove the stored token file

    await beeper.accounts()            # connected networks/accounts, as a polars frame
    await beeper.chats()               # chats across all accounts, newest activity first
    await beeper.messages(chat_id)     # recent messages in one chat
    await beeper.search("dinner")      # literal word search across all messages
    await beeper.search_chats("alice") # search chats by title/participant/network
    await beeper.send(chat_id, "hi")   # send a text message

Each read call returns a polars DataFrame with a fixed schema so empty results
stay typed. Credentials are per-user and never shared: the access token is read
from the ``BEEPER_ACCESS_TOKEN`` environment variable, or from a user-only file
at ``~/.config/beeper/token`` (written mode 0600 by :func:`login`). No token is
baked into the repo. Mint one in Beeper Desktop under Settings -> Integrations
("Approved connections"). The API base URL can be overridden with
``BEEPER_DESKTOP_BASE_URL`` (e.g. a custom port or a tunneled remote desktop).

The API is local-first: the server runs inside Beeper Desktop and binds to
localhost, so :exc:`BeeperError` from a connection failure usually means Beeper
Desktop is not running or the Desktop API is not enabled.

Beeper messages are the signed-in user's personal data across every network, so
this module is confined to **incognito sessions**: in a shared (multiplayer)
room (``IX_MCP_SHARED`` set) every data call raises before any network request,
so personal chats never reach state other participants can see.
"""

from __future__ import annotations

import os
import pathlib
import urllib.parse
from typing import Any

import httpx
import polars as pl

__all__ = [
    "BeeperError",
    "accounts",
    "chats",
    "login",
    "logout",
    "messages",
    "search",
    "search_chats",
    "send",
    "status",
]

__version__ = "0.1.0"

# The env var a shared (multiplayer) room sets on the one MCP it replicates
# across participants. Incognito is the default: an unset (or empty) value means
# access is permitted; only a truthy value marks the session shared and refuses
# access, keeping personal Beeper data out of synced room state.
SHARED_ENV = "IX_MCP_SHARED"

# Environment variables checked for an access token, in order.
_TOKEN_ENV_VARS = ("BEEPER_ACCESS_TOKEN",)

# The per-user token file path (mode 0600).
_TOKEN_FILE = pathlib.Path.home() / ".config" / "beeper" / "token"

# Where the Beeper Desktop API listens, and the env var that overrides it.
_BASE_URL_ENV = "BEEPER_DESKTOP_BASE_URL"
_DEFAULT_BASE_URL = "http://localhost:23373"

# Per-request timeout (seconds). The API is local, so a slow response usually
# means Beeper Desktop is busy indexing rather than a network stall.
_TIMEOUT = 30.0

# Fixed schemas so empty results stay typed.
_ACCOUNTS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "account_id": pl.Utf8,
    "network": pl.Utf8,
    "type": pl.Utf8,
    "provider": pl.Utf8,
    "status": pl.Utf8,
    "user_id": pl.Utf8,
    "full_name": pl.Utf8,
    "username": pl.Utf8,
    "phone": pl.Utf8,
    "is_self": pl.Boolean,
}

_CHATS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "account_id": pl.Utf8,
    "network": pl.Utf8,
    "type": pl.Utf8,
    "title": pl.Utf8,
    "unread_count": pl.Int64,
    "is_muted": pl.Boolean,
    "is_pinned": pl.Boolean,
    "last_activity": pl.Utf8,
    "preview_sender": pl.Utf8,
    "preview_text": pl.Utf8,
}

_MESSAGES_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "chat_id": pl.Utf8,
    "account_id": pl.Utf8,
    "sender_id": pl.Utf8,
    "is_sender": pl.Boolean,
    "timestamp": pl.Utf8,
    "type": pl.Utf8,
    "text": pl.Utf8,
    "reply_to": pl.Utf8,
    "attachments": pl.Int64,
}

_SEARCH_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "chat_id": pl.Utf8,
    "chat_title": pl.Utf8,
    "sender_id": pl.Utf8,
    "is_sender": pl.Boolean,
    "timestamp": pl.Utf8,
    "type": pl.Utf8,
    "text": pl.Utf8,
}

_SEARCH_CHATS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "account_id": pl.Utf8,
    "network": pl.Utf8,
    "type": pl.Utf8,
    "title": pl.Utf8,
    "unread_count": pl.Int64,
    "last_activity": pl.Utf8,
}


class BeeperError(RuntimeError):
    """Raised when the Beeper Desktop API cannot be reached for this session.

    Usually means "not configured" (call ``beeper.login(token)`` to store an
    access token) or "Beeper Desktop is not running" (the local API at
    ``http://localhost:23373`` refused the connection). Also raised in a shared
    room (where personal Beeper access is refused) and on API errors.
    """


def _require_incognito() -> None:
    """Refuse to access Beeper data in a shared (multiplayer) room.

    Beeper aggregates DMs and group chats across every connected network, so a
    shared room would leak one person's messages into state everyone can see. A
    shared room sets ``IX_MCP_SHARED``; only then is access refused.
    """
    if os.environ.get(SHARED_ENV):
        raise BeeperError(
            "Beeper is not available in a shared (multiplayer) room "
            "(IX_MCP_SHARED is set), because it would expose personal chats "
            "across every connected network to everyone in the room. Use it "
            "from an incognito chat instead; its transcript stays private to you."
        )


def _base_url() -> str:
    """The Beeper Desktop API base URL (no trailing slash)."""
    val = os.environ.get(_BASE_URL_ENV, "").strip()
    return (val or _DEFAULT_BASE_URL).rstrip("/")


def _token() -> str:
    """Return the access token, or raise BeeperError if none is configured.

    Resolution order: ``BEEPER_ACCESS_TOKEN`` env, then ``~/.config/beeper/token``
    (written by :func:`login`).
    """
    for var in _TOKEN_ENV_VARS:
        val = os.environ.get(var, "").strip()
        if val:
            return val
    if _TOKEN_FILE.exists():
        val = _TOKEN_FILE.read_text().strip()
        if val:
            return val
    raise BeeperError(
        "No Beeper access token is configured for this session. "
        "Call `beeper.login(token)` with an access token minted in Beeper "
        "Desktop (Settings -> Integrations -> Approved connections), set the "
        "BEEPER_ACCESS_TOKEN environment variable, or run `beeper.status()` to "
        "check the current state."
    )


async def _request(
    method: str,
    path: str,
    *,
    params: dict[str, Any] | None = None,
    json_body: dict[str, Any] | None = None,
) -> httpx.Response:
    """Call the Beeper Desktop API and return the response, or raise BeeperError.

    The access token goes in an ``Authorization: Bearer`` header (never the URL),
    so it stays out of logs. ``params`` values may be lists (encoded as repeated
    query params, which the API expects for its array filters). Raises
    :exc:`BeeperError` on a refused connection (Desktop not running), an HTTP
    error status, or a transport failure.
    """
    token = _token()
    url = f"{_base_url()}{path}"
    try:
        async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
            resp = await client.request(
                method,
                url,
                params=params,
                json=json_body,
                headers={"Authorization": f"Bearer {token}"},
            )
    except httpx.ConnectError as exc:
        raise BeeperError(
            f"Could not connect to the Beeper Desktop API at {_base_url()}. "
            "Make sure Beeper Desktop is running and the Desktop API is enabled "
            "(Settings -> Integrations), or set BEEPER_DESKTOP_BASE_URL."
        ) from exc
    except httpx.HTTPError as exc:
        raise BeeperError(f"Beeper Desktop API request failed for {path}: {exc}") from exc

    if resp.status_code in (401, 403):
        raise BeeperError(
            f"Beeper access token was rejected (HTTP {resp.status_code}). "
            "Call `beeper.login(token)` with a fresh token from Beeper Desktop "
            "(Settings -> Integrations -> Approved connections)."
        )
    if resp.status_code >= 400:
        raise BeeperError(
            f"Beeper Desktop API error for {path}: HTTP {resp.status_code} {resp.text[:200]}"
        )
    return resp


def _quote_id(chat_id: str) -> str:
    """URL-encode a chat ID for use as a path segment.

    Beeper chat IDs are Matrix-style (e.g. ``!whatsapp_…:beeper.com``) and carry
    ``!``, ``:``, and ``/`` that must not be read as path structure.
    """
    return urllib.parse.quote(chat_id, safe="")


def login(token: str) -> dict[str, Any]:
    """Store a Beeper access token for this user.

    Writes ``token`` to ``~/.config/beeper/token`` with mode 0600 so only this
    user can read it. Mint the token in Beeper Desktop under
    Settings -> Integrations -> Approved connections. Returns
    ``{"configured": True, "path": str}``.

    Call ``beeper.status()`` afterwards to confirm the token reaches a running
    Beeper Desktop.
    """
    _require_incognito()
    token = token.strip()
    if not token:
        raise BeeperError("token must not be empty")
    _TOKEN_FILE.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    # Write atomically: write to a temp file, chmod, then rename.
    tmp = _TOKEN_FILE.with_suffix(".tmp")
    try:
        tmp.write_text(token)
        tmp.chmod(0o600)
        tmp.rename(_TOKEN_FILE)
    except Exception:
        tmp.unlink(missing_ok=True)
        raise
    return {"configured": True, "path": str(_TOKEN_FILE)}


def logout() -> dict[str, Any]:
    """Remove the stored Beeper token file.

    Idempotent: returns ``{"signed_out": True, "removed": bool}`` whether or not
    the file existed. Does not revoke the token in Beeper Desktop.
    """
    removed = _TOKEN_FILE.exists()
    _TOKEN_FILE.unlink(missing_ok=True)
    return {"signed_out": True, "removed": removed}


async def status() -> dict[str, Any]:
    """Whether this session can reach Beeper Desktop, and which server.

    Returns ``{"configured": bool, "base_url": str, "version": str | None}`` and
    never raises: a missing/invalid token or an unreachable Desktop is reported
    as ``configured=False``, not an exception. Calls ``GET /v1/info``.

    Does not check the shared-room guard (it reads only server metadata, never
    personal chats), so it is safe to call in any session.
    """
    base = _base_url()
    try:
        resp = await _request("GET", "/v1/info")
    except BeeperError:
        return {"configured": False, "base_url": base, "version": None}
    info: dict[str, Any] = resp.json()
    return {
        "configured": True,
        "base_url": base,
        "version": info.get("version"),
    }


async def accounts() -> pl.DataFrame:
    """Connected Beeper accounts (one per network), as a polars DataFrame.

    Columns: ``account_id``, ``network`` (e.g. ``"WhatsApp"``), ``type`` (bridge
    type), ``provider`` (``"cloud"`` / ``"local"`` / ...), ``status`` (e.g.
    ``"connected"``), ``user_id``, ``full_name``, ``username``, ``phone``,
    ``is_self``.

    Use ``account_id`` to scope :func:`chats` or :func:`search`. Raises
    :exc:`BeeperError` when no token is configured, Beeper Desktop is
    unreachable, or in a shared room.
    """
    _require_incognito()
    resp = await _request("GET", "/v1/accounts")
    items: list[dict[str, Any]] = resp.json()
    rows: list[dict[str, Any]] = []
    for acct in items:
        bridge: dict[str, Any] = acct.get("bridge") or {}
        user: dict[str, Any] = acct.get("user") or {}
        rows.append(
            {
                "account_id": acct.get("accountID", ""),
                "network": acct.get("network", "") or "",
                "type": bridge.get("type", "") or "",
                "provider": bridge.get("provider", "") or "",
                "status": acct.get("status", "") or "",
                "user_id": user.get("id", "") or "",
                "full_name": user.get("fullName", "") or "",
                "username": user.get("username", "") or "",
                "phone": user.get("phoneNumber", "") or "",
                "is_self": bool(user.get("isSelf")),
            }
        )
    if not rows:
        return pl.DataFrame(schema=_ACCOUNTS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_ACCOUNTS_SCHEMA).select(list(_ACCOUNTS_SCHEMA))


async def _paginate(
    path: str,
    *,
    limit: int,
    params: dict[str, Any] | None = None,
) -> list[dict[str, Any]]:
    """Collect up to ``limit`` items from a cursor-paginated list endpoint.

    Beeper list endpoints return ``{items, hasMore, oldestCursor, newestCursor}``
    with the newest page first. We walk backwards (``direction="before"`` from
    ``oldestCursor``) until we have ``limit`` items or the server reports no more.
    """
    out: list[dict[str, Any]] = []
    cursor: str | None = None
    base_params = dict(params or {})
    while len(out) < limit:
        page_params = dict(base_params)
        if cursor:
            page_params["cursor"] = cursor
            page_params["direction"] = "before"
        resp = await _request("GET", path, params=page_params)
        data: dict[str, Any] = resp.json()
        items: list[dict[str, Any]] = data.get("items") or []
        out.extend(items)
        cursor = data.get("oldestCursor")
        if not data.get("hasMore") or not cursor or not items:
            break
    return out[:limit]


async def chats(*, limit: int = 50, account_id: str | None = None) -> pl.DataFrame:
    """Chats across all accounts, most recent activity first, as a polars DataFrame.

    Columns: ``id``, ``account_id``, ``network``, ``type`` (``"single"`` /
    ``"group"``), ``title``, ``unread_count``, ``is_muted``, ``is_pinned``,
    ``last_activity`` (ISO timestamp), ``preview_sender``, ``preview_text`` (the
    last message preview, when available).

    ``limit`` caps the rows returned (the API paginates automatically). Pass
    ``account_id`` to restrict to one account (see :func:`accounts`). Read a
    chat's messages with ``await beeper.messages(id)``. Raises :exc:`BeeperError`
    when no token is configured, Beeper Desktop is unreachable, or in a shared
    room.
    """
    _require_incognito()
    params: dict[str, Any] = {}
    if account_id:
        params["accountID"] = [account_id]
    items = await _paginate("/v1/chats", limit=limit, params=params)
    rows: list[dict[str, Any]] = []
    for chat in items:
        preview: dict[str, Any] = chat.get("preview") or {}
        rows.append(
            {
                "id": chat.get("id", ""),
                "account_id": chat.get("accountID", "") or "",
                "network": chat.get("network", "") or "",
                "type": chat.get("type", "") or "",
                "title": chat.get("title", "") or "",
                "unread_count": int(chat.get("unreadCount") or 0),
                "is_muted": bool(chat.get("isMuted")),
                "is_pinned": bool(chat.get("isPinned")),
                "last_activity": chat.get("lastActivity", "") or "",
                "preview_sender": preview.get("senderID", "") or "",
                "preview_text": preview.get("text", "") or "",
            }
        )
    if not rows:
        return pl.DataFrame(schema=_CHATS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_CHATS_SCHEMA).select(list(_CHATS_SCHEMA))


async def messages(chat_id: str, *, limit: int = 50) -> pl.DataFrame:
    """Recent messages in ``chat_id`` as a polars DataFrame, oldest row first.

    ``chat_id`` is a Beeper chat ID (or a local chat ID from this Desktop
    installation); get one from :func:`chats` or :func:`search`. Columns: ``id``,
    ``chat_id``, ``account_id``, ``sender_id``, ``is_sender`` (True when you sent
    it), ``timestamp`` (ISO), ``type`` (e.g. ``"TEXT"``), ``text``, ``reply_to``
    (the message ID this replies to, if any), ``attachments`` (count).

    ``limit`` caps the rows returned (the API paginates automatically). Raises
    :exc:`BeeperError` when no token is configured, Beeper Desktop is
    unreachable, or in a shared room.
    """
    _require_incognito()
    items = await _paginate(f"/v1/chats/{_quote_id(chat_id)}/messages", limit=limit)
    rows: list[dict[str, Any]] = [
        {
            "id": msg.get("id", ""),
            "chat_id": msg.get("chatID", "") or "",
            "account_id": msg.get("accountID", "") or "",
            "sender_id": msg.get("senderID", "") or "",
            "is_sender": bool(msg.get("isSender")),
            "timestamp": msg.get("timestamp", "") or "",
            "type": msg.get("type", "") or "",
            "text": msg.get("text", "") or "",
            "reply_to": msg.get("linkedMessageID", "") or "",
            "attachments": len(msg.get("attachments") or []),
        }
        for msg in items
    ]
    if not rows:
        return pl.DataFrame(schema=_MESSAGES_SCHEMA)
    # The API returns newest-first while paginating; present chronologically.
    return (
        pl.DataFrame(rows, schema_overrides=_MESSAGES_SCHEMA)
        .select(list(_MESSAGES_SCHEMA))
        .sort("timestamp")
    )


async def search(
    query: str | None = None,
    *,
    limit: int = 20,
    account_id: str | None = None,
    chat_id: str | None = None,
    sender: str | None = None,
    date_after: str | None = None,
    date_before: str | None = None,
) -> pl.DataFrame:
    """Search messages across all chats and return matches as a polars DataFrame.

    ``query`` is a literal word search (non-semantic): it finds messages
    containing those exact words in any order. Use single words people actually
    type (``"dinner"``, not ``"dinner plans"``). Omit ``query`` to filter purely
    by the other parameters.

    Columns: ``chat_id``, ``chat_title``, ``sender_id``, ``is_sender``,
    ``timestamp``, ``type``, ``text``.

    Narrow with ``account_id`` / ``chat_id`` (a single id), ``sender`` (``"me"``,
    ``"others"``, or a user id), and ``date_after`` / ``date_before`` (ISO 8601,
    e.g. ``"2024-07-01T00:00:00Z"``). Raises :exc:`BeeperError` when no token is
    configured, Beeper Desktop is unreachable, or in a shared room.
    """
    _require_incognito()
    params: dict[str, Any] = {"limit": limit}
    if query:
        params["query"] = query
    if account_id:
        params["accountID"] = [account_id]
    if chat_id:
        params["chatID"] = [chat_id]
    if sender:
        params["sender"] = sender
    if date_after:
        params["dateAfter"] = date_after
    if date_before:
        params["dateBefore"] = date_before

    resp = await _request("GET", "/v1/messages/search", params=params)
    data: dict[str, Any] = resp.json()
    chat_map: dict[str, Any] = data.get("chats") or {}
    rows: list[dict[str, Any]] = []
    for msg in data.get("items") or []:
        cid = msg.get("chatID", "") or ""
        chat_info: dict[str, Any] = chat_map.get(cid) or {}
        rows.append(
            {
                "chat_id": cid,
                "chat_title": chat_info.get("title", "") or "",
                "sender_id": msg.get("senderID", "") or "",
                "is_sender": bool(msg.get("isSender")),
                "timestamp": msg.get("timestamp", "") or "",
                "type": msg.get("type", "") or "",
                "text": msg.get("text", "") or "",
            }
        )
        if len(rows) >= limit:
            break
    if not rows:
        return pl.DataFrame(schema=_SEARCH_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_SEARCH_SCHEMA).select(list(_SEARCH_SCHEMA))


async def search_chats(
    query: str | None = None,
    *,
    limit: int = 20,
    account_id: str | None = None,
    inbox: str | None = None,
) -> pl.DataFrame:
    """Search chats by title, network, or participant names, as a polars DataFrame.

    Columns: ``id``, ``account_id``, ``network``, ``type``, ``title``,
    ``unread_count``, ``last_activity``.

    Narrow with ``account_id`` and ``inbox`` (``"primary"`` / ``"low-priority"``
    / ``"archive"``). Raises :exc:`BeeperError` when no token is configured,
    Beeper Desktop is unreachable, or in a shared room.
    """
    _require_incognito()
    params: dict[str, Any] = {}
    if query:
        params["query"] = query
    if account_id:
        params["accountID"] = [account_id]
    if inbox:
        params["inbox"] = inbox
    items = await _paginate("/v1/chats/search", limit=limit, params=params)
    rows: list[dict[str, Any]] = [
        {
            "id": chat.get("id", ""),
            "account_id": chat.get("accountID", "") or "",
            "network": chat.get("network", "") or "",
            "type": chat.get("type", "") or "",
            "title": chat.get("title", "") or "",
            "unread_count": int(chat.get("unreadCount") or 0),
            "last_activity": chat.get("lastActivity", "") or "",
        }
        for chat in items
    ]
    if not rows:
        return pl.DataFrame(schema=_SEARCH_CHATS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_SEARCH_CHATS_SCHEMA).select(
        list(_SEARCH_CHATS_SCHEMA)
    )


async def send(chat_id: str, text: str, *, reply_to: str | None = None) -> dict[str, Any]:
    """Send ``text`` to ``chat_id`` and return the pending-message metadata.

    ``chat_id`` is a Beeper (or local) chat ID from :func:`chats` / :func:`search`.
    Pass ``reply_to`` (a message ID) to send the message as a reply. Returns
    ``{"chat_id": str, "pending_message_id": str}``; the network confirms the
    send asynchronously, so the ID is provisional.

    Beeper recommends the API for personal use only -- high send volume can get a
    network account suspended. Raises :exc:`BeeperError` on failure or in a
    shared room.
    """
    _require_incognito()
    body: dict[str, Any] = {"text": text}
    if reply_to:
        body["replyToMessageID"] = reply_to
    resp = await _request(
        "POST", f"/v1/chats/{_quote_id(chat_id)}/messages", json_body=body
    )
    data: dict[str, Any] = resp.json()
    return {
        "chat_id": data.get("chatID", "") or chat_id,
        "pending_message_id": data.get("pendingMessageID", "") or "",
    }
