"""Slack for the kernel: read channels, DMs, messages, and threads; send; search.

Bundled into the ix-mcp interpreter so a session can ``import slack`` with no
install step. Credentials are per-user and never shared: a Slack token is read
from the ``SLACK_USER_TOKEN`` or ``SLACK_TOKEN`` environment variable, or from a
user-only file at ``~/.config/slack/token`` (written mode 0600 by :func:`login`).
No token is baked into the repo.

    import slack

    slack.login("xoxp-...")           # store your token (written mode 0600)
    slack.status()                    # {"configured": True, "team": ..., "user": ...}
    slack.logout()                    # remove the stored token file

    await slack.channels()            # channels you can see, as a polars frame
    await slack.dms()                 # your direct-message conversations
    await slack.messages("general")   # recent messages in #general (incl. bots)
    await slack.messages("@hari")     # recent messages in your DM with @hari
    await slack.thread("general", "1234567890.123456")  # a single thread
    await slack.send("general", "hello from ix")        # post a message
    await slack.send("general", "in-thread reply", thread_ts="1234567890.123456")
    await slack.search("deploy staging")                # search across Slack

Each call returns a polars DataFrame with a fixed schema so empty results stay
typed. Raises :exc:`SlackError` when no token is configured; the message names
the next step (``slack.login(token)``).

The token's reach is whatever OAuth scopes the Slack app was granted, so a
search or DM read can fail with ``missing_scope``; the error names the scope to
add to the app (then re-mint the token). Common scopes: ``channels:history`` /
``groups:history`` / ``im:history`` (read messages), ``im:read`` (list DMs),
``search:read`` (search), ``chat:write`` (post).

Slack messages carry the signed-in user's personal data (DMs, private channels),
so this module is confined to **incognito sessions**: in a shared (multiplayer)
room (``IX_MCP_SHARED`` set) every call raises before any network request, so a
personal workspace never reaches state other participants can see.
"""

from __future__ import annotations

import json
import os
import pathlib
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

import polars as pl
from pydantic import BaseModel, ConfigDict

__all__ = [
    "SlackError",
    "channels",
    "dms",
    "login",
    "logout",
    "messages",
    "search",
    "send",
    "status",
    "thread",
]

__version__ = "0.3.0"

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

# Message subtypes that are pure channel-membership / housekeeping noise. These
# are dropped from `messages()` by default; everything else -- including
# `bot_message` (CI/deploy/webhook posts), `me_message`, `thread_broadcast`, and
# file shares -- is kept, so a bot-only channel no longer reads as empty. (The
# old code dropped every message with any subtype, which silently emptied
# channels whose traffic is all bots.)
_NOISE_SUBTYPES = frozenset(
    {
        "channel_join",
        "channel_leave",
        "channel_topic",
        "channel_purpose",
        "channel_name",
        "channel_archive",
        "channel_unarchive",
        "group_join",
        "group_leave",
        "group_topic",
        "group_purpose",
        "group_name",
        "group_archive",
        "group_unarchive",
        "pinned_item",
        "unpinned_item",
        "bot_add",
        "bot_remove",
        "reminder_add",
    }
)

# Fixed schemas so empty results stay typed.
_CHANNELS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "name": pl.Utf8,
    "is_private": pl.Boolean,
    "is_member": pl.Boolean,
    "num_members": pl.Int64,
    "topic": pl.Utf8,
    "purpose": pl.Utf8,
}

_DMS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "user_id": pl.Utf8,
    "user": pl.Utf8,
    "real_name": pl.Utf8,
}

_MESSAGES_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "ts": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "subtype": pl.Utf8,
    "reply_count": pl.Int64,
    "reactions": pl.Int64,
}

_THREAD_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "ts": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "subtype": pl.Utf8,
    "reply_count": pl.Int64,
}

_SEARCH_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "ts": pl.Utf8,
    "channel_id": pl.Utf8,
    "channel_name": pl.Utf8,
    "user": pl.Utf8,
    "text": pl.Utf8,
    "permalink": pl.Utf8,
}


class _SlackProfile(BaseModel):
    model_config = ConfigDict(extra="ignore")

    display_name: str | None = None
    real_name: str | None = None


class _SlackMember(BaseModel):
    model_config = ConfigDict(extra="ignore")

    id: str
    name: str | None = None
    profile: _SlackProfile | None = None


class _SlackImChannel(BaseModel):
    model_config = ConfigDict(extra="ignore")

    id: str


class _SlackChannel(BaseModel):
    model_config = ConfigDict(extra="ignore")

    id: str
    name: str | None = None


class SlackError(RuntimeError):
    """Raised when Slack cannot be reached for this session.

    Usually means "not configured": call ``slack.login(token)`` to store a
    Slack token. Also raised in a shared room (where personal Slack access is
    refused) and on API errors from the Slack Web API (a ``missing_scope`` error
    names the OAuth scope to add).
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
    """Return the Slack token, or raise SlackError if none is configured.

    Resolution order: ``SLACK_USER_TOKEN`` env, ``SLACK_TOKEN`` env, then
    ``~/.config/slack/token`` (written by :func:`login`).
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


def _api_call(method: str, token: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    """Call a Slack Web API method and return the decoded JSON response.

    Every call is a form POST with the token in an ``Authorization: Bearer``
    header (never in the URL or query string, so it stays out of server logs).
    A form POST also unifies reads, ``search``, and ``chat.postMessage`` through
    one path.

    Raises :exc:`SlackError` on HTTP errors or when Slack returns ``ok=false``;
    a ``missing_scope`` error is rewritten to name the scope to add.
    """
    body = urllib.parse.urlencode(params or {}).encode("utf-8")
    req = urllib.request.Request(  # noqa: S310 -- URL always https://slack.com/api/*, not user-supplied
        f"{_API_BASE}/{method}",
        data=body,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/x-www-form-urlencoded; charset=utf-8",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310
            raw = resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise SlackError(f"Slack API HTTP {exc.code} for {method}") from exc
    except urllib.error.URLError as exc:
        raise SlackError(f"Slack API request failed for {method}: {exc.reason}") from exc

    data: dict[str, Any] = json.loads(raw)
    if not data.get("ok"):
        error = data.get("error", "unknown_error")
        if error in ("invalid_auth", "not_authed", "token_revoked", "token_expired"):
            raise SlackError(
                f"Slack token is invalid or expired ({error}). "
                "Call `slack.login(token)` with a fresh token."
            )
        if error == "missing_scope":
            # Slack returns the exact scope it wanted and what the token has, so
            # surface both instead of a bare "missing_scope". (Granular scopes
            # like search:read.public do NOT satisfy search:read -- this names
            # the difference.)
            needed = data.get("needed") or "?"
            have = data.get("provided") or "?"
            raise SlackError(
                f"Slack API `{method}` needs the `{needed}` OAuth scope "
                f"(token has: {have}). Add `{needed}` to the Slack app's user "
                "scopes and re-mint the token."
            )
        raise SlackError(f"Slack API error for {method}: {error}")
    return data


def login(token: str) -> dict[str, Any]:
    """Store a Slack token for this user.

    Writes ``token`` to ``~/.config/slack/token`` with mode 0600 so only this
    user can read it. ``token`` is normally a user token (``xoxp-``); a bot
    token (``xoxb-``) also works for the methods its scopes allow. Returns
    ``{"configured": True, "path": str}``.

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


def logout() -> dict[str, Any]:
    """Remove the stored Slack token file.

    Idempotent: returns ``{"signed_out": True, "removed": bool}`` whether or not
    the file existed. Does not revoke the token at Slack.
    """
    removed = _TOKEN_FILE.exists()
    _TOKEN_FILE.unlink(missing_ok=True)
    return {"signed_out": True, "removed": removed}


def status() -> dict[str, Any]:
    """Whether this session has a Slack token configured, and as whom.

    Returns ``{"configured": bool, "team": str | None, "user": str | None}``
    and never raises: a missing or invalid token is reported as
    ``configured=False``, not an exception. Call ``slack.login(token)`` to
    configure.

    Does not check the shared-room guard (it only reads configuration, never
    personal data), so it is safe to call in any session.
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


def _users_map(token: str) -> dict[str, dict[str, str]]:
    """Return ``{user_id: {"name": ..., "real_name": ...}}`` for the workspace."""
    out: dict[str, dict[str, str]] = {}
    cursor: str | None = None
    while True:
        params: dict[str, Any] = {"limit": 200}
        if cursor:
            params["cursor"] = cursor
        data = _api_call("users.list", token, params)
        for u in data.get("members", []):
            prof: dict[str, Any] = u.get("profile") or {}
            out[u.get("id", "")] = {
                "name": u.get("name", "") or "",
                "real_name": (prof.get("real_name") or u.get("real_name") or ""),
            }
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    return out


def _resolve_user(name_or_id: str, token: str) -> str:
    """Return the user ID for ``name_or_id`` (a ``U…``/``W…`` id, or a username).

    A username is matched (case-insensitively) against the handle, display name,
    and real name. Raises :exc:`SlackError` if no user matches.
    """
    s = name_or_id.lstrip("@").strip()
    if s[:1] in ("U", "W") and len(s) >= 9 and s == s.upper():
        return s
    want = s.lower()
    cursor: str | None = None
    while True:
        params: dict[str, Any] = {"limit": 200}
        if cursor:
            params["cursor"] = cursor
        data = _api_call("users.list", token, params)
        for u in [_SlackMember.model_validate(m) for m in data.get("members", [])]:
            prof = u.profile
            names = {
                (u.name or "").lower(),
                (prof.display_name or "").lower() if prof else "",
                (prof.real_name or "").lower() if prof else "",
            }
            if want and want in names:
                return u.id
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    raise SlackError(
        f"No Slack user matched {name_or_id!r}. "
        "Use `await slack.dms()` to list your direct messages."
    )


def _open_im(user_id: str, token: str) -> str:
    """Return the DM channel ID for ``user_id`` (opening it if needed)."""
    data = _api_call("conversations.open", token, {"users": user_id})
    raw_channel: dict[str, Any] = data.get("channel") or {}
    channel = _SlackImChannel.model_validate(raw_channel)
    return channel.id


def _resolve_channel_by_name(name: str, token: str) -> str | None:
    """Return the channel ID for a ``#name`` / ``name``, or None if not found."""
    want = name.lstrip("#").lower()
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
        for ch in [_SlackChannel.model_validate(c) for c in data.get("channels", [])]:
            if (ch.name or "").lower() == want:
                return ch.id
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break
    return None


def _resolve_channel(channel: str, token: str) -> str:
    """Resolve ``channel`` to a conversation ID.

    Accepts a channel/group/DM ID, a ``#channel`` or bare channel name, a
    ``@username`` or user ID (resolved to the DM with that user), or a bare name
    that is a username when it is not a channel. Raises :exc:`SlackError` when
    nothing matches.
    """
    c = channel.strip()
    if not c:
        raise SlackError("channel must not be empty")

    # Explicit @user -> the DM with that user.
    if c.startswith("@"):
        return _open_im(_resolve_user(c, token), token)

    up = c.upper()
    if up[:1] in ("C", "G", "D") and len(c) >= 9 and c == up:
        return c  # already a channel / group / DM id
    if up[:1] in ("U", "W") and len(c) >= 9 and c == up:
        return _open_im(c, token)  # a user id -> the DM with that user

    # A bare name: try a channel first, then fall back to a username (DM).
    found = _resolve_channel_by_name(c, token)
    if found:
        return found
    try:
        return _open_im(_resolve_user(c, token), token)
    except SlackError:
        raise SlackError(
            f"No channel or user matched {channel!r}. Use `await slack.channels()` "
            "or `await slack.dms()` to list what you can see."
        ) from None


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
    returned (Slack paginates automatically). For direct messages prefer
    :func:`dms`, which also resolves the other person's name.

    Raises :exc:`SlackError` when no token is configured or in a shared room.
    """
    _require_incognito()
    token = _token()

    rows: list[dict[str, Any]] = []
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
        rows.extend(
            {
                "id": ch.get("id", ""),
                "name": ch.get("name", ""),
                "is_private": bool(ch.get("is_private")),
                "is_member": bool(ch.get("is_member")),
                "num_members": int(ch.get("num_members") or 0),
                "topic": (ch.get("topic") or {}).get("value", "") or "",
                "purpose": (ch.get("purpose") or {}).get("value", "") or "",
            }
            for ch in data.get("channels", [])
        )
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_CHANNELS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_CHANNELS_SCHEMA).select(
        list(_CHANNELS_SCHEMA)
    )


async def dms(*, limit: int = 100) -> pl.DataFrame:
    """Your direct-message conversations, as a polars DataFrame.

    Columns: ``id`` (the ``D…`` channel id), ``user_id``, ``user`` (handle),
    ``real_name``. Read one with ``await slack.messages("@<user>")`` or
    ``await slack.messages(id)``.

    Listing DMs needs the ``im:read`` scope. Names are resolved with
    ``users:read`` when available; without it ``user``/``real_name`` come back
    blank rather than failing the call. Raises :exc:`SlackError` when no token is
    configured, ``im:read`` is missing (the error names it), or in a shared room.
    """
    _require_incognito()
    token = _token()
    # Listing IMs needs only im:read; resolving names needs users:read. Degrade
    # to blank names rather than failing the whole call when users:read is absent.
    try:
        umap = _users_map(token)
    except SlackError:
        umap = {}

    rows: list[dict[str, Any]] = []
    cursor: str | None = None
    while len(rows) < limit:
        params: dict[str, Any] = {"types": "im", "limit": min(200, limit - len(rows))}
        if cursor:
            params["cursor"] = cursor
        data = _api_call("conversations.list", token, params)
        for ch in data.get("channels", []):
            uid = ch.get("user", "") or ""
            info = umap.get(uid, {})
            rows.append(
                {
                    "id": ch.get("id", ""),
                    "user_id": uid,
                    "user": info.get("name", ""),
                    "real_name": info.get("real_name", ""),
                }
            )
        cursor = (data.get("response_metadata") or {}).get("next_cursor") or ""
        if not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_DMS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_DMS_SCHEMA).select(list(_DMS_SCHEMA))


async def messages(
    channel: str,
    *,
    limit: int = 50,
    include_noise: bool = False,
) -> pl.DataFrame:
    """Recent messages in ``channel`` as a polars DataFrame.

    ``channel`` may be a channel ID or name (``"general"`` / ``"#general"``), a
    ``@username`` or user ID (the DM with that user), or a ``D…`` DM id.

    Columns: ``ts`` (Slack timestamp string), ``user`` (or the bot's name/id for
    bot posts), ``text``, ``subtype`` (empty for ordinary messages,
    ``"bot_message"`` for CI/deploy/webhook posts, etc.), ``reply_count``,
    ``reactions`` (total reaction count).

    Bot and other content-bearing messages are **kept**; only channel-membership
    and housekeeping subtypes are dropped, so a bot-only channel no longer reads
    as empty. Pass ``include_noise=True`` to keep those too.

    Reading needs the matching history scope (``channels:history`` /
    ``groups:history`` / ``im:history`` / ``mpim:history``). Resolving a
    ``@user``/user-id to a DM uses ``conversations.open`` (needs ``im:write``);
    pass a ``D…`` id or use :func:`dms` to avoid that. Raises :exc:`SlackError`
    when no token is configured, the conversation is not found, or in a shared
    room.
    """
    _require_incognito()
    token = _token()
    channel_id = _resolve_channel(channel, token)

    data = _api_call(
        "conversations.history",
        token,
        {"channel": channel_id, "limit": min(limit, 1000)},
    )
    rows: list[dict[str, Any]] = []
    for msg in data.get("messages", []):
        sub = msg.get("subtype") or ""
        if not include_noise and sub in _NOISE_SUBTYPES:
            continue
        reactions = sum(r.get("count", 0) for r in msg.get("reactions", []))
        rows.append(
            {
                "ts": msg.get("ts", ""),
                # Ordinary messages carry `user`; bot posts carry `username` /
                # `bot_id` instead, so fall back rather than emit a blank.
                "user": msg.get("user") or msg.get("username") or msg.get("bot_id") or "",
                "text": msg.get("text", ""),
                "subtype": sub,
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

    ``channel`` is resolved like :func:`messages` (channel, ``@user``, or id);
    ``ts`` is the Slack timestamp of the parent message (e.g.
    ``"1234567890.123456"``). Columns: ``ts``, ``user``, ``text``, ``subtype``,
    ``reply_count``.

    Raises :exc:`SlackError` when no token is configured, the conversation is not
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
    rows: list[dict[str, Any]] = []
    # No noise filter here (unlike messages()): a thread's replies are content by
    # definition and rarely carry channel-membership subtypes, so keep them all.
    for msg in data.get("messages", []):
        rows.append(
            {
                "ts": msg.get("ts", ""),
                "user": msg.get("user") or msg.get("username") or msg.get("bot_id") or "",
                "text": msg.get("text", ""),
                "subtype": msg.get("subtype") or "",
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


async def send(
    channel: str,
    text: str,
    *,
    thread_ts: str | None = None,
    reply_broadcast: bool = False,
) -> dict[str, Any]:
    """Post ``text`` to ``channel`` and return Slack's response metadata.

    ``channel`` is resolved like :func:`messages` (channel, ``@user``, or id).

    Pass ``thread_ts`` -- the Slack timestamp of a parent message (the ``ts``
    from :func:`messages` or :func:`thread`, e.g. ``"1234567890.123456"``) -- to
    reply *inside* that thread instead of posting a new top-level message. Set
    ``reply_broadcast=True`` to also surface a threaded reply to the whole
    channel; it is only valid with ``thread_ts`` and raises otherwise.

    Returns ``{"ok": True, "ts": "<timestamp>", "channel": "<id>",
    "thread_ts": "<parent ts, or "">"}`` on success (``thread_ts`` is non-empty
    for a threaded reply). Needs ``chat:write``. Raises :exc:`SlackError` on
    failure or in a shared room.
    """
    _require_incognito()
    if reply_broadcast and not thread_ts:
        raise SlackError("reply_broadcast=True is only valid together with thread_ts")
    token = _token()
    channel_id = _resolve_channel(channel, token)

    params: dict[str, Any] = {"channel": channel_id, "text": text}
    if thread_ts:
        params["thread_ts"] = thread_ts
        if reply_broadcast:
            params["reply_broadcast"] = "true"

    data = _api_call("chat.postMessage", token, params)
    # chat.postMessage echoes the stored message; a threaded reply carries its
    # parent's `thread_ts`, so surface it (empty for a top-level post).
    posted: dict[str, Any] = data.get("message") or {}
    return {
        "ok": True,
        "ts": data.get("ts", ""),
        "channel": data.get("channel", ""),
        "thread_ts": posted.get("thread_ts", "") or "",
    }


async def search(
    query: str,
    *,
    limit: int = 20,
) -> pl.DataFrame:
    """Search Slack for ``query`` and return matching messages as a polars DataFrame.

    Columns: ``ts``, ``channel_id``, ``channel_name``, ``user``, ``text``,
    ``permalink``.

    Search needs the ``search:read`` scope on a user token (bot tokens cannot
    search; the granular ``search:read.*`` scopes do **not** satisfy
    ``search.messages``). Raises :exc:`SlackError` when no token is configured,
    the scope is missing (the error names it), or in a shared room.
    """
    _require_incognito()
    token = _token()

    data = _api_call(
        "search.messages",
        token,
        {"query": query, "count": min(limit, 100), "sort": "timestamp"},
    )
    matches = (data.get("messages") or {}).get("matches", [])
    rows: list[dict[str, Any]] = []
    for msg in matches:
        channel: dict[str, Any] = msg.get("channel") or {}
        is_dict = isinstance(channel, dict)
        rows.append(
            {
                "ts": msg.get("ts", ""),
                "channel_id": channel.get("id", "") if is_dict else "",
                "channel_name": channel.get("name", "") if is_dict else "",
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
