"""Slack for the kernel: read channels, messages, and threads; send messages; search.

Bundled into the ix-mcp interpreter so a session can ``import slack`` with no
install step. Credentials are per-user and never shared: a Slack user token is
read from the ``SLACK_USER_TOKEN`` or ``SLACK_TOKEN`` environment variable, or
from a user-only file at ``~/.config/slack/token`` (written mode 0600 by
:func:`login`). No token is baked into the repo.

    import slack

    slack.login("xoxp-...")          # store your token (written mode 0600)
    slack.status()                   # {"configured": True, "team": ..., "user": ...}
    slack.logout()                   # remove the stored token file

    await slack.channels()           # all channels you can see, as a polars frame
    await slack.messages("general")  # recent messages in #general
    await slack.thread("general", "1234567890.123456")  # a single thread
    await slack.send("general", "hello from ix")        # post a message
    await slack.search("deploy staging")                # search across Slack

Each call returns a polars DataFrame with a fixed schema so empty results stay
typed. Raises :exc:`SlackError` when no token is configured; the message names
the next step (``slack.login(token)``).

Slack messages carry the signed-in user's personal data (DMs, private channels),
so this module is confined to **incognito sessions**: in a shared (multiplayer)
room (``IX_MCP_SHARED`` set) every call raises before any network request, so a
personal workspace never reaches state other participants can see.
"""

from __future__ import annotations

import json
import os
import pathlib
import stat
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

import polars as pl

__all__ = [
    "SlackError",
    "channels",
    "login",
    "logout",
    "messages",
    "search",
    "send",
    "status",
    "thread",
]

__version__ = "0.1.0"

# The env var a shared (multiplayer) room sets on the one MCP it replicates
# across participants. Incognito is the default: an unset (or empty) value means
# access is permitted; only a truthy value marks the session shared and refuses
# access, keeping personal Slack data out of synced room state.
SHARED_ENV = "IX_MCP_SHARED"

# Environment variables checked for a token, in order.
_TOKEN_ENV_VARS = ("SLACK_USER_TOKEN", "SLACK_TOKEN")

# The per-user token file path (mode 0600).
_TOKEN_FILE = pathlib.Path.home() / ".config" / "slack" / "token"

# Slack Web API base URL.
_API_BASE = "https://slack.com/api"

# Fixed schemas so empty results stay typed.
_CHANNELS_SCHEMA: dict[str, pl.DataType] = {
    "id": pl.Utf8,
    "name": pl.Utf8,
    "is_private": pl.Boolean,
    "is_member": pl.Boolean,
    "num_members": pl.Int64,
    "topic": pl.Utf8,
    "purpose": pl.Utf8,
}

_MESSAGES_SCHEMA: dict[str, pl.DataType] = {
    "ts": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "reply_count": pl.Int64,
    "reactions": pl.Int64,
}

_THREAD_SCHEMA: dict[str, pl.DataType] = {
    "ts": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "reply_count": pl.Int64,
}

_SEARCH_SCHEMA: dict[str, pl.DataType] = {
    "ts": pl.Utf8,
    "channel_id": pl.Utf8,
    "channel_name": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "permalink": pl.Utf8,
}


class SlackError(RuntimeError):
    """Raised when Slack cannot be reached for this session.

    Usually means "not configured": call ``slack.login(token)`` to store a
    Slack user token. Also raised in a shared room (where personal Slack access
    is refused) and on API errors from the Slack Web API.
    """


def _require_incognito() -> None:
    """Refuse to access Slack data in a shared (multiplayer) room.

    Slack messages include DMs and private channel history, so a shared room
    would leak one person's workspace into state everyone can see. A shared room
    sets ``IX_MCP_SHARED``; only then is access refused.
    """
    if os.environ.get(SHARED_ENV):
        raise SlackError(
            "Slack is not available in a shared (multiplayer) room "
            "(IX_MCP_SHARED is set), because it would expose personal Slack "
            "messages and channels to everyone in the room. Use it from an "
            "incognito chat instead; its transcript stays private to you."
        )


def _token() -> str:
    """Return the Slack user token, or raise SlackError if none is configured.

    Resolution order:
    1. ``SLACK_USER_TOKEN`` env var
    2. ``SLACK_TOKEN`` env var
    3. ``~/.config/slack/token`` (written by :func:`login`)
    """
    for var in _TOKEN_ENV_VARS:
        val = os.environ.get(var, "").strip()
        if val:
            return val
    if _TOKEN_FILE.exists():
        val = _TOKEN_FILE.read_text().strip()
        if val:
            return val
    raise SlackError(
        "No Slack token is configured for this session. "
        "Call `slack.login(token)` with your Slack user token "
        "(starts with `xoxp-`), set the SLACK_USER_TOKEN environment "
        "variable, or run `slack.status()` to check the current state."
    )


def _api_call(method: str, token: str, params: dict[str, Any] | None = None) -> dict:
    """Call a Slack Web API method and return the decoded JSON response.

    Raises :exc:`SlackError` on HTTP errors or when Slack returns ``ok=false``.
    The token is passed as a Bearer header and never placed in the URL or query
    string so it stays out of server logs.
    """
    url = f"{_API_BASE}/{method}"
    qs = urllib.parse.urlencode(params or {})
    if qs:
        url = f"{url}?{qs}"

    req = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json; charset=utf-8",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise SlackError(f"Slack API HTTP {exc.code} for {method}") from exc
    except urllib.error.URLError as exc:
        raise SlackError(f"Slack API request failed for {method}: {exc.reason}") from exc

    data = json.loads(body)
    if not data.get("ok"):
        error = data.get("error", "unknown_error")
        if error in ("invalid_auth", "not_authed", "token_revoked", "token_expired"):
            raise SlackError(
                f"Slack token is invalid or expired ({error}). "
                "Call `slack.login(token)` with a fresh token."
            )
        raise SlackError(f"Slack API error for {method}: {error}")
    return data


def login(token: str) -> dict:
    """Store a Slack user token for this user.

    Writes ``token`` to ``~/.config/slack/token`` with mode 0600 so only this
    user can read it. The token must be a Slack user token (starts with
    ``xoxp-``). Returns ``{"configured": True, "path": str}`` on success.

    Call ``slack.status()`` afterwards to confirm the token is valid.
    """
    _require_incognito()
    token = token.strip()
    if not token:
        raise SlackError("token must not be empty")
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


def logout() -> dict:
    """Remove the stored Slack token file.

    Idempotent: returns ``{"signed_out": True}`` whether or not the file
    existed. Does not revoke the token at Slack.
    """
    removed = _TOKEN_FILE.exists()
    if removed:
        _TOKEN_FILE.unlink()
    return {"signed_out": True, "removed": removed}


def status() -> dict:
    """Whether this session has a Slack token configured, and as whom.

    Returns ``{"configured": bool, "team": str | None, "user": str | None}``
    and never raises: a missing or invalid token is reported as
    ``configured=False``, not an exception. Call ``slack.login(token)`` to
    configure.

    Note: this function does not check the shared-room guard (it only reads
    configuration, never personal data), so it is safe to call in any session.
    """
    try:
        tok = _token()
    except SlackError:
        return {"configured": False, "team": None, "user": None}
    try:
        data = _api_call("auth.test", tok)
        return {
            "configured": True,
            "team": data.get("team"),
            "user": data.get("user"),
        }
    except SlackError:
        return {"configured": False, "team": None, "user": None}


def _resolve_channel(channel: str, token: str) -> str:
    """Return the channel ID for ``channel``.

    Accepts an existing channel ID (starts with ``C`` or ``D`` or ``G``),
    or a channel name (with or without a leading ``#``). On a name miss, tries
    the conversations.list API to resolve it. Returns the ID unchanged if it
    already looks like one, or raises ``SlackError`` if not found.
    """
    # Already looks like a Slack ID.
    if channel.upper().startswith(("C", "D", "G")) and len(channel) >= 9:
        return channel

    name = channel.lstrip("#").lower()
    cursor: str | None = None
    while True:
        params: dict[str, Any] = {
            "types": "public_channel,private_channel",
            "exclude_archived": "true",
            "limit": 200,
        }
        if cursor:
            params["cursor"] = cursor
        data = _api_call("conversations.list", token, params)
        for ch in data.get("channels", []):
            if ch.get("name", "").lower() == name:
                return ch["id"]
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    raise SlackError(
        f"Channel {channel!r} not found. "
        "Use `await slack.channels()` to list channels you can see."
    )


async def channels(
    *,
    types: str = "public_channel,private_channel",
    limit: int = 200,
) -> pl.DataFrame:
    """All Slack channels this token can see, as a polars DataFrame.

    Columns: ``id``, ``name``, ``is_private``, ``is_member``, ``num_members``,
    ``topic``, ``purpose``.

    Pass ``types`` to narrow to ``"public_channel"``, ``"private_channel"``,
    ``"mpim"``, or ``"im"`` (comma-separated). ``limit`` caps the total rows
    returned (Slack paginates automatically).

    Raises :exc:`SlackError` when no token is configured or in a shared room.
    """
    _require_incognito()
    token = _token()

    rows: list[dict] = []
    cursor: str | None = None
    while len(rows) < limit:
        params: dict[str, Any] = {
            "types": types,
            "exclude_archived": "true",
            "limit": min(200, limit - len(rows)),
        }
        if cursor:
            params["cursor"] = cursor
        data = _api_call("conversations.list", token, params)
        for ch in data.get("channels", []):
            rows.append(
                {
                    "id": ch.get("id", ""),
                    "name": ch.get("name", ""),
                    "is_private": bool(ch.get("is_private")),
                    "is_member": bool(ch.get("is_member")),
                    "num_members": int(ch.get("num_members") or 0),
                    "topic": (ch.get("topic") or {}).get("value", "") or "",
                    "purpose": (ch.get("purpose") or {}).get("value", "") or "",
                }
            )
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_CHANNELS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_CHANNELS_SCHEMA).select(
        list(_CHANNELS_SCHEMA)
    )


async def messages(
    channel: str,
    *,
    limit: int = 50,
) -> pl.DataFrame:
    """Recent messages in ``channel`` as a polars DataFrame.

    ``channel`` may be a channel ID or a name (``"general"`` or ``"#general"``).
    Columns: ``ts`` (Slack timestamp string), ``user``, ``text``,
    ``reply_count``, ``reactions`` (total reaction count).

    Raises :exc:`SlackError` when no token is configured, the channel is not
    found, or in a shared room.
    """
    _require_incognito()
    token = _token()
    channel_id = _resolve_channel(channel, token)

    data = _api_call(
        "conversations.history",
        token,
        {"channel": channel_id, "limit": min(limit, 1000)},
    )
    rows: list[dict] = []
    for msg in data.get("messages", []):
        if msg.get("subtype"):
            continue
        reactions = sum(r.get("count", 0) for r in msg.get("reactions", []))
        rows.append(
            {
                "ts": msg.get("ts", ""),
                "user": msg.get("user", "") or msg.get("bot_id", ""),
                "text": msg.get("text", ""),
                "reply_count": int(msg.get("reply_count") or 0),
                "reactions": int(reactions),
            }
        )
        if len(rows) >= limit:
            break

    if not rows:
        return pl.DataFrame(schema=_MESSAGES_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_MESSAGES_SCHEMA).select(
        list(_MESSAGES_SCHEMA)
    )


async def thread(
    channel: str,
    ts: str,
    *,
    limit: int = 100,
) -> pl.DataFrame:
    """Messages in a single thread as a polars DataFrame.

    ``channel`` may be a channel ID or name; ``ts`` is the Slack timestamp of
    the parent message (e.g. ``"1234567890.123456"``). Columns: ``ts``,
    ``user``, ``text``, ``reply_count``.

    Raises :exc:`SlackError` when no token is configured, the channel is not
    found, or in a shared room.
    """
    _require_incognito()
    token = _token()
    channel_id = _resolve_channel(channel, token)

    data = _api_call(
        "conversations.replies",
        token,
        {"channel": channel_id, "ts": ts, "limit": min(limit, 1000)},
    )
    rows: list[dict] = []
    for msg in data.get("messages", []):
        rows.append(
            {
                "ts": msg.get("ts", ""),
                "user": msg.get("user", "") or msg.get("bot_id", ""),
                "text": msg.get("text", ""),
                "reply_count": int(msg.get("reply_count") or 0),
            }
        )
        if len(rows) >= limit:
            break

    if not rows:
        return pl.DataFrame(schema=_THREAD_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_THREAD_SCHEMA).select(
        list(_THREAD_SCHEMA)
    )


async def send(channel: str, text: str) -> dict:
    """Post ``text`` to ``channel`` and return Slack's response metadata.

    ``channel`` may be a channel ID or name (``"general"`` or ``"#general"``).
    Returns ``{"ok": True, "ts": "<timestamp>", "channel": "<id>"}`` on
    success. Raises :exc:`SlackError` on failure or in a shared room.
    """
    _require_incognito()
    token = _token()
    channel_id = _resolve_channel(channel, token)

    req = urllib.request.Request(
        f"{_API_BASE}/chat.postMessage",
        data=json.dumps({"channel": channel_id, "text": text}).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json; charset=utf-8",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310
            body = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise SlackError(f"Slack API HTTP {exc.code} for chat.postMessage") from exc
    except urllib.error.URLError as exc:
        raise SlackError(
            f"Slack API request failed for chat.postMessage: {exc.reason}"
        ) from exc

    data = json.loads(body)
    if not data.get("ok"):
        error = data.get("error", "unknown_error")
        if error in ("invalid_auth", "not_authed", "token_revoked", "token_expired"):
            raise SlackError(
                f"Slack token is invalid or expired ({error}). "
                "Call `slack.login(token)` with a fresh token."
            )
        raise SlackError(f"Slack API error for chat.postMessage: {error}")
    return {"ok": True, "ts": data.get("ts", ""), "channel": data.get("channel", "")}


async def search(
    query: str,
    *,
    limit: int = 20,
) -> pl.DataFrame:
    """Search Slack for ``query`` and return matching messages as a polars DataFrame.

    Columns: ``ts``, ``channel_id``, ``channel_name``, ``user``, ``text``,
    ``permalink``.

    Raises :exc:`SlackError` when no token is configured or in a shared room.
    Note: search requires a user token (``xoxp-``); bot tokens cannot search.
    """
    _require_incognito()
    token = _token()

    data = _api_call(
        "search.messages",
        token,
        {"query": query, "count": min(limit, 100), "sort": "timestamp"},
    )
    matches = (data.get("messages") or {}).get("matches", [])
    rows: list[dict] = []
    for msg in matches:
        channel = msg.get("channel") or {}
        rows.append(
            {
                "ts": msg.get("ts", ""),
                "channel_id": channel.get("id", "") if isinstance(channel, dict) else "",
                "channel_name": channel.get("name", "") if isinstance(channel, dict) else "",
                "user": msg.get("user", "") or msg.get("username", ""),
                "text": msg.get("text", ""),
                "permalink": msg.get("permalink", ""),
            }
        )
        if len(rows) >= limit:
            break

    if not rows:
        return pl.DataFrame(schema=_SEARCH_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_SEARCH_SCHEMA).select(
        list(_SEARCH_SCHEMA)
    )
