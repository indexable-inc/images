"""Beeper for the kernel: read accounts, chats, and messages across every network; search; send.

Beeper Desktop exposes a fully local HTTP API (default ``http://localhost:23373``)
that aggregates chats from WhatsApp, Telegram, Signal, iMessage, Instagram,
Messenger, Discord, Slack, X, LinkedIn, Google Messages, and more. This module
bundles a thin, polars-shaped wrapper so a session can ``import beeper`` with no
install step.

    import beeper

    beeper.login("<access token>")     # store your token (written mode 0600)
    await beeper.status()              # {"configured": True, "base_url": ..., "version": ...}
    beeper.logout()                    # remove the stored token file

    await beeper.accounts()            # connected networks/accounts, as a polars frame
    await beeper.chats()               # chats across all accounts, newest activity first
    await beeper.messages(chat_id)     # recent messages in one chat
    await beeper.search("dinner")      # literal word search across all messages
    await beeper.search_chats("alice") # search chats by title/participant/network
    await beeper.send(chat_id, "hi")   # send a text message

Each read call returns a polars DataFrame with a fixed schema so empty results
stay typed. Credentials are per-user and never shared: the access token is read
from the ``BEEPER_ACCESS_TOKEN`` (or ``BEEPER_API_TOKEN``) environment variable,
or from a user-only file at ``~/.config/beeper/token`` (written mode 0600 by
:func:`login`). No token is
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
from pydantic import BaseModel, ConfigDict, Field

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

# Environment variables checked for an access token, in order. BEEPER_ACCESS_TOKEN
# is the name the official Beeper CLI/SDK use; BEEPER_API_TOKEN is also accepted
# as a common alias.
_TOKEN_ENV_VARS = ("BEEPER_ACCESS_TOKEN", "BEEPER_API_TOKEN")

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

# Timestamp columns are parsed from the API's ISO 8601 strings into a real
# tz-aware Datetime so callers can do native polars time math (filter by date,
# group_by_dynamic, sort) instead of string compares.
_TS = pl.Datetime(time_unit="us", time_zone="UTC")

_CHATS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "account_id": pl.Utf8,
    "network": pl.Utf8,
    "type": pl.Utf8,
    "title": pl.Utf8,
    "unread_count": pl.Int64,
    "is_muted": pl.Boolean,
    "is_pinned": pl.Boolean,
    "last_activity": _TS,
    "preview_sender": pl.Utf8,
    "preview_text": pl.Utf8,
}

_MESSAGES_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "chat_id": pl.Utf8,
    "account_id": pl.Utf8,
    "sender_id": pl.Utf8,
    "is_sender": pl.Boolean,
    "timestamp": _TS,
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
    "timestamp": _TS,
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
    "last_activity": _TS,
}


# Pydantic models for the API response objects: validate-and-default the raw
# JSON (``extra="ignore"`` drops fields we don't surface; aliases map the API's
# camelCase to snake attributes) so the puller bodies read typed fields instead
# of fragile ``dict.get(..., "") or ""`` chains. Timestamps stay ``str`` here so
# `_frame` remains the single place datetimes are parsed.
class _ApiModel(BaseModel):
    # extra="ignore" drops API fields we don't surface. Fields use
    # ``validation_alias`` (not ``alias``) so the API's camelCase keys are read on
    # model_validate while the field keeps its snake name with a default -- which
    # keeps each model a zero-arg-constructable type for ``default_factory``.
    model_config = ConfigDict(extra="ignore")


class _Bridge(_ApiModel):
    type: str = ""
    provider: str = ""


class _User(_ApiModel):
    id: str = ""
    full_name: str = Field("", validation_alias="fullName")
    username: str = ""
    phone_number: str = Field("", validation_alias="phoneNumber")
    is_self: bool = Field(default=False, validation_alias="isSelf")


class _Account(_ApiModel):
    account_id: str = Field("", validation_alias="accountID")
    network: str = ""
    status: str = ""
    bridge: _Bridge = Field(default_factory=_Bridge)
    user: _User | None = None


class _Message(_ApiModel):
    id: str = ""
    chat_id: str = Field("", validation_alias="chatID")
    account_id: str = Field("", validation_alias="accountID")
    sender_id: str = Field("", validation_alias="senderID")
    is_sender: bool = Field(default=False, validation_alias="isSender")
    timestamp: str = ""
    type: str = ""
    text: str = ""
    linked_message_id: str = Field("", validation_alias="linkedMessageID")
    attachments: list[dict[str, Any]] = Field(default_factory=list)


class _Chat(_ApiModel):
    id: str = ""
    account_id: str = Field("", validation_alias="accountID")
    network: str = ""
    type: str = ""
    title: str = ""
    unread_count: int = Field(0, validation_alias="unreadCount")
    is_muted: bool = Field(default=False, validation_alias="isMuted")
    is_pinned: bool = Field(default=False, validation_alias="isPinned")
    last_activity: str = Field("", validation_alias="lastActivity")
    preview: _Message | None = None


class _ListPage(_ApiModel):
    """A cursor-paginated list envelope; items stay raw for the caller's model."""

    items: list[dict[str, Any]] = Field(default_factory=list)
    has_more: bool = Field(default=False, validation_alias="hasMore")
    oldest_cursor: str | None = Field(None, validation_alias="oldestCursor")


class _SearchPage(_ApiModel):
    """The message-search envelope: typed messages plus a chatID->chat map."""

    chats: dict[str, _Chat] = Field(default_factory=dict)
    items: list[_Message] = Field(default_factory=list)
    has_more: bool = Field(default=False, validation_alias="hasMore")
    oldest_cursor: str | None = Field(None, validation_alias="oldestCursor")


class _AppInfo(_ApiModel):
    version: str | None = None


class _Info(_ApiModel):
    app: _AppInfo = Field(default_factory=_AppInfo)
    version: str | None = None  # older builds expose it at the top level


class _SendResult(_ApiModel):
    chat_id: str = Field("", validation_alias="chatID")
    pending_message_id: str = Field("", validation_alias="pendingMessageID")


def _frame(
    rows: list[dict[str, Any]],
    schema: dict[str, pl.DataType | type[pl.DataType]],
) -> pl.DataFrame:
    """Build a typed, column-ordered frame from API rows.

    Empty input returns the bare schema so downstream chains keep working on no
    results. Datetime columns are parsed from ISO 8601 strings (``strict=False``
    so an unparseable value becomes null rather than raising); everything else is
    cast straight to its declared dtype.

    When a datetime column has no parseable value (every row empty/missing) we
    emit a typed null column instead of calling ``str.to_datetime``: with no
    sample, polars' format inference raises ``ComputeError`` rather than nulling
    out -- and ``strict=False`` only nulls *individual* failures, not that.
    """
    if not rows:
        return pl.DataFrame(schema=schema)
    df = pl.DataFrame(rows)
    exprs: list[pl.Expr] = []
    for name, dtype in schema.items():
        present = name in df.columns
        if isinstance(dtype, pl.Datetime):
            has_value = present and bool(
                (df.get_column(name).cast(pl.Utf8).str.strip_chars().str.len_chars().fill_null(0) > 0).any()
            )
            if has_value:
                exprs.append(
                    pl.col(name).cast(pl.Utf8).str.to_datetime(time_zone="UTC", strict=False).alias(name)
                )
            else:
                exprs.append(pl.lit(None, dtype=dtype).alias(name))
        else:
            col = pl.col(name) if present else pl.lit(None)
            exprs.append(col.cast(dtype).alias(name))
    return df.select(exprs)


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

    Resolution order: ``BEEPER_ACCESS_TOKEN`` env, ``BEEPER_API_TOKEN`` env, then
    ``~/.config/beeper/token`` (written by :func:`login`).
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
        "BEEPER_ACCESS_TOKEN (or BEEPER_API_TOKEN) environment variable, or run "
        "`beeper.status()` to check the current state."
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
        # trust_env=False: every request carries the user's bearer token to a
        # local API, so never honor HTTP_PROXY/ALL_PROXY -- on a host with proxy
        # env vars set, httpx would otherwise route the Authorization header to
        # that proxy instead of keeping it on localhost.
        async with httpx.AsyncClient(timeout=_TIMEOUT, trust_env=False) as client:
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
    # Write atomically and never world-readable: create the temp file 0600 from
    # the first open (O_EXCL avoids reusing an attacker-planted file), write, then
    # rename over the final path. (Path.write_text would create it with the
    # process umask first and only chmod afterwards, briefly exposing the token.)
    tmp = _TOKEN_FILE.with_suffix(".tmp")
    try:
        tmp.unlink(missing_ok=True)
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as handle:
            handle.write(token)
        tmp.replace(_TOKEN_FILE)
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

    In a shared (multiplayer) room it reports ``configured=False`` WITHOUT
    reading or sending the token: ``_request`` would send the bearer token to
    ``BEEPER_DESKTOP_BASE_URL``, which another participant could point at a
    listener they control, so the token must not leave an incognito session.
    """
    base = _base_url()
    if os.environ.get(SHARED_ENV):
        return {"configured": False, "base_url": base, "version": None}
    try:
        resp = await _request("GET", "/v1/info")
    except BeeperError:
        return {"configured": False, "base_url": base, "version": None}
    info = _Info.model_validate(resp.json())
    # /v1/info nests the app version under `app.version`; fall back to a
    # top-level `version` for older builds.
    return {
        "configured": True,
        "base_url": base,
        "version": info.app.version or info.version,
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
    rows: list[dict[str, Any]] = [
        {
            "account_id": a.account_id,
            "network": a.network,
            "type": a.bridge.type,
            "provider": a.bridge.provider,
            "status": a.status,
            "user_id": a.user.id if a.user else "",
            "full_name": a.user.full_name if a.user else "",
            "username": a.user.username if a.user else "",
            "phone": a.user.phone_number if a.user else "",
            "is_self": a.user.is_self if a.user else False,
        }
        for a in (_Account.model_validate(d) for d in resp.json())
    ]
    return _frame(rows, _ACCOUNTS_SCHEMA)


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
        page = _ListPage.model_validate(resp.json())
        out.extend(page.items)
        cursor = page.oldest_cursor
        if not page.has_more or not cursor or not page.items:
            break
    return out[:limit]


async def chats(*, limit: int = 50, account_id: str | None = None) -> pl.DataFrame:
    """Chats across all accounts, most recent activity first, as a polars DataFrame.

    Columns: ``id``, ``account_id``, ``network``, ``type`` (``"single"`` /
    ``"group"``), ``title``, ``unread_count``, ``is_muted``, ``is_pinned``,
    ``last_activity`` (tz-aware UTC datetime), ``preview_sender``, ``preview_text`` (the
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
        params["accountIDs"] = [account_id]
    raw = await _paginate("/v1/chats", limit=limit, params=params)
    rows: list[dict[str, Any]] = [
        {
            "id": c.id,
            "account_id": c.account_id,
            "network": c.network,
            "type": c.type,
            "title": c.title,
            "unread_count": c.unread_count,
            "is_muted": c.is_muted,
            "is_pinned": c.is_pinned,
            "last_activity": c.last_activity,
            "preview_sender": c.preview.sender_id if c.preview else "",
            "preview_text": c.preview.text if c.preview else "",
        }
        for c in (_Chat.model_validate(d) for d in raw)
    ]
    return _frame(rows, _CHATS_SCHEMA)


async def messages(chat_id: str, *, limit: int = 50) -> pl.DataFrame:
    """Recent messages in ``chat_id`` as a polars DataFrame, oldest row first.

    ``chat_id`` is a Beeper chat ID (or a local chat ID from this Desktop
    installation); get one from :func:`chats` or :func:`search`. Columns: ``id``,
    ``chat_id``, ``account_id``, ``sender_id``, ``is_sender`` (True when you sent
    it), ``timestamp`` (tz-aware UTC datetime), ``type`` (e.g. ``"TEXT"``), ``text``, ``reply_to``
    (the message ID this replies to, if any), ``attachments`` (count).

    ``limit`` caps the rows returned (the API paginates automatically). Raises
    :exc:`BeeperError` when no token is configured, Beeper Desktop is
    unreachable, or in a shared room.
    """
    _require_incognito()
    raw = await _paginate(f"/v1/chats/{_quote_id(chat_id)}/messages", limit=limit)
    rows: list[dict[str, Any]] = [
        {
            "id": m.id,
            "chat_id": m.chat_id,
            "account_id": m.account_id,
            "sender_id": m.sender_id,
            "is_sender": m.is_sender,
            "timestamp": m.timestamp,
            "type": m.type,
            "text": m.text,
            "reply_to": m.linked_message_id,
            "attachments": len(m.attachments),
        }
        for m in (_Message.model_validate(d) for d in raw)
    ]
    # The API returns newest-first while paginating; present chronologically.
    return _frame(rows, _MESSAGES_SCHEMA).sort("timestamp")


async def search(
    query: str | None = None,
    *,
    limit: int = 20,
    account_id: str | None = None,
    chat_id: str | None = None,
    sender: str | None = None,
    date_after: str | None = None,
    date_before: str | None = None,
    exclude_low_priority: bool = False,
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
    e.g. ``"2024-07-01T00:00:00Z"``). ``exclude_low_priority`` defaults to
    ``False`` so low-priority-inbox messages are included (the Beeper API itself
    defaults this flag to true and would otherwise drop them silently); pass
    ``True`` for a more refined search. Raises :exc:`BeeperError` when no token is
    configured, Beeper Desktop is unreachable, or in a shared room.
    """
    _require_incognito()
    base_params: dict[str, Any] = {"excludeLowPriority": exclude_low_priority}
    if query:
        base_params["query"] = query
    if account_id:
        base_params["accountIDs"] = [account_id]
    if chat_id:
        base_params["chatIDs"] = [chat_id]
    if sender:
        base_params["sender"] = sender
    if date_after:
        base_params["dateAfter"] = date_after
    if date_before:
        base_params["dateBefore"] = date_before

    # /v1/messages/search returns one cursor-paginated page per call (a single
    # page is capped well below large limits), so page oldest-ward until we have
    # `limit` matches, accumulating the chatID->chat map across pages for titles.
    items: list[_Message] = []
    chat_map: dict[str, _Chat] = {}
    cursor: str | None = None
    while len(items) < limit:
        params = dict(base_params)
        if cursor:
            params["cursor"] = cursor
            params["direction"] = "before"
        resp = await _request("GET", "/v1/messages/search", params=params)
        page = _SearchPage.model_validate(resp.json())
        chat_map.update(page.chats)
        items.extend(page.items)
        cursor = page.oldest_cursor
        if not page.has_more or not cursor or not page.items:
            break

    titles = {cid: c.title for cid, c in chat_map.items()}
    rows: list[dict[str, Any]] = [
        {
            "chat_id": m.chat_id,
            "chat_title": titles.get(m.chat_id, ""),
            "sender_id": m.sender_id,
            "is_sender": m.is_sender,
            "timestamp": m.timestamp,
            "type": m.type,
            "text": m.text,
        }
        for m in items[:limit]
    ]
    return _frame(rows, _SEARCH_SCHEMA)


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
        params["accountIDs"] = [account_id]
    if inbox:
        params["inbox"] = inbox
    raw = await _paginate("/v1/chats/search", limit=limit, params=params)
    rows: list[dict[str, Any]] = [
        {
            "id": c.id,
            "account_id": c.account_id,
            "network": c.network,
            "type": c.type,
            "title": c.title,
            "unread_count": c.unread_count,
            "last_activity": c.last_activity,
        }
        for c in (_Chat.model_validate(d) for d in raw)
    ]
    return _frame(rows, _SEARCH_CHATS_SCHEMA)


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
    result = _SendResult.model_validate(resp.json())
    return {
        "chat_id": result.chat_id or chat_id,
        "pending_message_id": result.pending_message_id,
    }
