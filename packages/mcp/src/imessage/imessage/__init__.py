"""Native macOS iMessage access for the ix-mcp interpreter: read to polars, send.

Bundled into the pinned interpreter the same way ``screen`` and ``vmkit`` are, so
every session can ``import imessage`` with no install step. macOS keeps the whole
Messages history in a SQLite database (``~/Library/Messages/chat.db``) and the
address book in another (``~/Library/Application Support/AddressBook``); this
module reads both into ``polars`` DataFrames, sends new messages through the
Messages app over AppleScript, and edits contacts through the Contacts app.

    import imessage

    df = imessage.messages(limit=500)        # recent messages as a polars frame
    df = imessage.messages(contact="Alana")  # just one conversation (by name…)
    df = imessage.messages(contact="+12025550123")  # …or by phone / email

    imessage.chats()                          # one row per conversation, newest first
    imessage.contacts()                       # the address book as a polars frame

    imessage.send("+12025550123", "on my way")   # send an iMessage
    imessage.send("a@b.com", "hi", service="SMS")  # …or a green-bubble SMS

    cid = imessage.add_contact(first="Ada", last="Lovelace", phones=["+12025550123"])
    imessage.update_contact("Ada Lovelace", last="King", add_emails=["ada@example.com"])
    imessage.delete_contact(cid)             # by id or unique name

Reading is the interesting half. Modern macOS stores most message text not in
``message.text`` (often NULL) but in ``message.attributedBody``, an archived
``NSAttributedString``; :func:`messages` decodes it with Foundation's
``NSUnarchiver`` and falls back to ``text`` so the ``text`` column is always the
plain string a human typed. Apple stores timestamps as nanoseconds since
2001-01-01 UTC (Core Data epoch); the ``date`` column is converted to a real
UTC-aware ``datetime``. Handles (the phone/email on the other end) are resolved
to contact names via the address book, so the ``name`` column reads like the
Messages app.

macOS permissions

Reading ``chat.db`` requires the host process to have **Full Disk Access**
(System Settings > Privacy & Security > Full Disk Access); without it SQLite
fails to open the file and :func:`messages` raises a clear error pointing here.
Sending drives the Messages app through AppleScript, which the first time prompts
for **Automation** permission to control "Messages"; grant it and retry. Contact
edits likewise drive the Contacts app (first use prompts for Automation
permission to control "Contacts"); they save into the default account, so on a
Mac signed into iCloud they sync to every device like an edit made by hand.

This module is macOS-only (the databases and the Messages app are Apple's);
importing on a non-Darwin platform raises ``RuntimeError``.
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import subprocess
import sys
from collections.abc import Iterable
from datetime import datetime, timezone

import polars as pl

__all__ = [
    "CHAT_DB",
    "contacts",
    "add_contact",
    "update_contact",
    "delete_contact",
    "chats",
    "messages",
    "send",
]

if sys.platform != "darwin":
    raise RuntimeError(
        "imessage: the Messages and Contacts databases are macOS-only "
        f"(running on {sys.platform!r})."
    )

# The standard location of the Messages SQLite database. Reading it needs Full
# Disk Access for the host process; opened read-only + immutable so a live
# Messages app writing to it cannot block or be disturbed by our reads.
CHAT_DB = os.path.expanduser("~/Library/Messages/chat.db")

# Core Data / Apple epoch: 2001-01-01 UTC, in nanoseconds since the Unix epoch.
# `message.date` is nanoseconds since this point on modern macOS (it was *seconds*
# before High Sierra); both are handled in `_to_datetime`.
_APPLE_EPOCH = datetime(2001, 1, 1, tzinfo=timezone.utc)
_APPLE_EPOCH_NS = int(_APPLE_EPOCH.timestamp() * 1_000_000_000)


def _connect(path: str) -> sqlite3.Connection:
    """Open a SQLite database read-only, with a clear error.

    Opened ``mode=ro`` (not ``immutable=1``): the Messages database runs in WAL
    mode, where ``immutable`` makes SQLite ignore the ``-wal`` file and so miss
    just-written rows (a message you sent a moment ago stays invisible until a
    checkpoint). A plain read-only connection reads the WAL too, so reads are
    fresh, and WAL allows a reader alongside the Messages app's writer without
    contending for a lock. A failure here is almost always a missing Full Disk
    Access grant, so say so rather than surface a bare sqlite error.
    """

    if not os.path.exists(path):
        raise FileNotFoundError(
            f"imessage: no database at {path!r}. On macOS this lives under your "
            "home Library; pass an explicit `db=` path if it has moved."
        )
    try:
        return sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.OperationalError as exc:  # pragma: no cover - permission wiring
        raise PermissionError(
            f"imessage: could not open {path!r} ({exc}). Reading the Messages "
            "and Contacts databases requires Full Disk Access: grant the process "
            "running ix-mcp access under System Settings > Privacy & Security > "
            "Full Disk Access, then restart it."
        ) from exc


def _decode_attributed_body(blob: bytes | None) -> str | None:
    """Decode an archived ``NSAttributedString`` blob to its plain text.

    Modern macOS leaves ``message.text`` NULL and stores the message in
    ``message.attributedBody`` as a ``streamtyped`` ``NSArchiver`` payload.
    Foundation's ``NSUnarchiver`` is the canonical decoder for that format, so we
    let Apple's own code reconstruct the string instead of byte-scraping the blob.
    """

    if not blob:
        return None
    import Foundation

    try:
        data = Foundation.NSData.dataWithBytes_length_(blob, len(blob))
        obj = Foundation.NSUnarchiver.unarchiveObjectWithData_(data)
    except Exception:
        return None
    if obj is None:
        return None
    string = obj.string() if hasattr(obj, "string") else obj
    return str(string)


def _norm(handle: str | None) -> str | None:
    """Normalize a phone/email so a handle and a contact entry compare equal.

    Emails fold to lowercase; phone numbers keep their digits and compare on the
    last 10 (so ``+1 (202) 555-0123`` matches ``2025550123``), which is enough to
    line up Messages handles with address-book numbers without a full libphonenumber.
    """

    if not handle:
        return None
    handle = handle.strip()
    if "@" in handle:
        return handle.lower()
    digits = re.sub(r"\D", "", handle)
    return digits[-10:] if len(digits) >= 10 else digits or None


def _to_datetime(col: str) -> pl.Expr:
    """A polars expr turning an Apple ``date`` column into a UTC datetime.

    Values are nanoseconds since 2001-01-01 on modern macOS and seconds on older
    systems; the magnitude tells them apart (a seconds value never reaches 1e11).
    """

    ns = pl.col(col)
    unified_ns = pl.when(ns.abs() < 1_000_000_000_00).then(ns * 1_000_000_000).otherwise(ns)
    return pl.from_epoch(unified_ns + _APPLE_EPOCH_NS, time_unit="ns").dt.replace_time_zone("UTC")


def _contact_db() -> str | None:
    """The newest address-book SQLite database, or None if there is none.

    macOS shards the address book per source under ``Sources/<uuid>/`` and also
    keeps a top-level database; the most recently modified one is the live store.
    """

    import glob

    root = os.path.expanduser("~/Library/Application Support/AddressBook")
    candidates = glob.glob(os.path.join(root, "AddressBook-v22.abcddb"))
    candidates += glob.glob(os.path.join(root, "Sources/*/AddressBook-v22.abcddb"))
    candidates = [p for p in candidates if os.path.exists(p)]
    if not candidates:
        return None
    return max(candidates, key=os.path.getmtime)


_CONTACT_SCHEMA = {
    "id": pl.Utf8, "name": pl.Utf8, "first": pl.Utf8, "last": pl.Utf8,
    "organization": pl.Utf8, "phones": pl.List(pl.Utf8), "emails": pl.List(pl.Utf8),
}


def contacts(*, db: str | None = None) -> pl.DataFrame:
    """The macOS address book as a polars DataFrame, one row per contact.

    Columns: ``id`` (the stable address-book unique id, e.g.
    ``"UUID:ABPerson"`` -- the same id :func:`update_contact` and
    :func:`delete_contact` accept), ``name`` (full display name), ``first``,
    ``last``, ``organization``, ``phones`` (list of strings), ``emails`` (list of
    strings). Pass ``db`` to read a specific ``.abcddb`` file; by default the
    newest address-book store is used. Reading requires Full Disk Access (see the
    module docstring).
    """

    path = db or _contact_db()
    if path is None:
        return pl.DataFrame(schema=_CONTACT_SCHEMA)
    con = _connect(path)
    try:
        records = con.execute(
            "SELECT Z_PK, ZUNIQUEID, ZFIRSTNAME, ZLASTNAME, ZORGANIZATION, ZNICKNAME FROM ZABCDRECORD"
        ).fetchall()
        phones: dict[int, list[str]] = {}
        for owner, number in con.execute(
            "SELECT ZOWNER, ZFULLNUMBER FROM ZABCDPHONENUMBER WHERE ZFULLNUMBER IS NOT NULL"
        ):
            phones.setdefault(owner, []).append(number)
        emails: dict[int, list[str]] = {}
        for owner, addr in con.execute(
            "SELECT ZOWNER, ZADDRESS FROM ZABCDEMAILADDRESS WHERE ZADDRESS IS NOT NULL"
        ):
            emails.setdefault(owner, []).append(addr)
    finally:
        con.close()

    rows = []
    for pk, uid, first, last, org, nick in records:
        name = " ".join(p for p in (first, last) if p) or nick or org
        ph, em = phones.get(pk, []), emails.get(pk, [])
        if not (name or ph or em):
            continue
        rows.append({
            "id": uid, "name": name, "first": first, "last": last,
            "organization": org, "phones": ph, "emails": em,
        })
    return pl.DataFrame(rows, schema=_CONTACT_SCHEMA).sort("name", nulls_last=True)


def _name_index(contact_db: str | None) -> dict[str, str]:
    """Map every normalized handle (phone/email) to its contact display name."""

    try:
        df = contacts(db=contact_db)
    except (FileNotFoundError, PermissionError):
        return {}
    index: dict[str, str] = {}
    for row in df.iter_rows(named=True):
        name = row["name"]
        if not name:
            continue
        for value in (row["phones"] or []) + (row["emails"] or []):
            key = _norm(value)
            if key:
                index.setdefault(key, name)
    return index


def _resolve_to_handles(con: sqlite3.Connection, contact: str, contact_db: str | None) -> list[int]:
    """The ``handle.ROWID`` values that belong to ``contact``.

    ``contact`` may be a phone number, an email, or a contact name. A name is
    looked up in the address book and expanded to that person's numbers/emails,
    so ``messages(contact="Alana")`` works the same as passing her number.
    """

    wanted = {_norm(contact)}
    looks_like_handle = "@" in contact or any(ch.isdigit() for ch in contact)
    if not looks_like_handle:
        needle = contact.strip().lower()
        try:
            df = contacts(db=contact_db)
        except (FileNotFoundError, PermissionError):
            df = pl.DataFrame()
        for row in df.iter_rows(named=True):
            if needle in (row["name"] or "").lower():
                for value in (row["phones"] or []) + (row["emails"] or []):
                    wanted.add(_norm(value))
    wanted.discard(None)
    return [rid for rid, hid in con.execute("SELECT ROWID, id FROM handle") if _norm(hid) in wanted]


def _apple_ns(when: datetime | str) -> int:
    """A datetime/ISO-string converted to Apple nanoseconds for a ``date`` filter."""

    if isinstance(when, str):
        when = datetime.fromisoformat(when)
    if when.tzinfo is None:
        when = when.replace(tzinfo=timezone.utc)
    return int(when.timestamp() * 1_000_000_000) - _APPLE_EPOCH_NS


# A reply or tapback references another message by GUID, sometimes prefixed with
# the message *part* it points at: "p:<part>/<guid>" for one part of a balloon,
# "bp:<guid>" for the whole balloon. Only the bare GUID joins back to `message.guid`.
_ASSOC_PREFIX = re.compile(r"^(?:p:\d+/|bp:)")


def _bare_guid(value: str | None) -> str | None:
    """The bare message GUID from a reply/tapback reference, any part prefix stripped."""

    if not value:
        return None
    return _ASSOC_PREFIX.sub("", value)


# `associated_message_type` encodes a tapback: 2000-2007 add one, 3000-3007 retract
# the matching one. The low digit is the reaction; the thousands digit is add (2)
# vs. remove (3), so one table keyed on the offset covers both.
_TAPBACKS = {
    0: "loved", 1: "liked", 2: "disliked", 3: "laughed",
    4: "emphasized", 5: "questioned", 6: "emoji", 7: "sticker",
}


def _tapback(assoc_type: int | None) -> str | None:
    """A tapback label for `associated_message_type`, or None for a real message.

    ``0`` is an ordinary message; ``2000``-``2007`` are tapbacks and
    ``3000``-``3007`` retract the matching one (``"removed-loved"``).
    """

    if not assoc_type:
        return None
    base, offset = divmod(assoc_type, 1000)
    if base not in (2, 3):  # only 2xxx (add) and 3xxx (remove) are tapbacks
        return None
    name = _TAPBACKS.get(offset)
    if name is None:
        return None
    return f"removed-{name}" if base == 3 else name


# These message columns arrived in later macOS releases (thread_originator_guid in
# macOS 11, date_edited / date_retracted in macOS 13), so an older chat.db lacks
# them. Each is selected only when present, with NULL otherwise, so reading a
# legacy database degrades to empty reply/tapback/edit fields instead of raising
# "no such column".
_OPTIONAL_COLUMNS = (
    ("thread_originator_guid", "reply_to_raw"),
    ("associated_message_guid", "assoc_guid"),
    ("associated_message_type", "assoc_type"),
    ("date_edited", "date_edited"),
    ("date_retracted", "date_retracted"),
)


def _optional_columns(con: sqlite3.Connection) -> str:
    """The SELECT fragment for version-gated message columns, NULL where absent."""

    present = {row[1] for row in con.execute("PRAGMA table_info(message)")}
    indent = ",\n" + " " * 19
    return indent.join(
        f"m.{name} AS {alias}" if name in present else f"NULL AS {alias}"
        for name, alias in _OPTIONAL_COLUMNS
    )


def messages(
    *,
    contact: str | None = None,
    chat: str | None = None,
    from_me: bool | None = None,
    since: datetime | str | None = None,
    until: datetime | str | None = None,
    limit: int = 2000,
    resolve_names: bool = True,
    db: str | None = None,
) -> pl.DataFrame:
    """Messages from ``chat.db`` as a polars DataFrame, newest first.

    Columns: ``rowid``, ``guid``, ``date`` (UTC datetime), ``name`` (resolved
    contact name or None), ``handle`` (phone/email of the other party),
    ``is_from_me`` (bool), ``text`` (decoded from ``attributedBody`` when needed),
    ``reply_to_guid`` / ``reply_to_rowid`` / ``reply_to_text`` (the message this
    one is a threaded reply to, resolved; all None when it is not a reply),
    ``tapback`` (``"loved"`` / ``"removed-liked"`` / ... for a reaction, else
    None) and ``tapback_target_guid`` (the message it reacts to), ``edited`` and
    ``unsent`` (bool, from ``date_edited`` / ``date_retracted``), ``service``
    (``iMessage`` / ``SMS``), ``chat_id``, ``chat_name``, ``chat_identifier``,
    ``is_read`` (bool), ``n_attachments``.

    Filters (all optional):
      * ``contact`` -- a phone, email, or contact name (a name is expanded to that
        person's handles via the address book).
      * ``chat`` -- a conversation's display name or identifier (for group chats).
      * ``from_me`` -- True for only your messages, False for only received.
      * ``since`` / ``until`` -- a ``datetime`` or ISO date string bound.
      * ``limit`` -- max rows (most recent), default 2000.

    Set ``resolve_names=False`` to skip the address-book join (the ``name`` column
    is then all None) when you only need handles. Reading requires Full Disk
    Access (see the module docstring).
    """

    con = _connect(db or CHAT_DB)
    try:
        clauses, params = [], []
        if contact is not None:
            handle_ids = _resolve_to_handles(con, contact, None)
            if not handle_ids:
                return _empty_messages()
            clauses.append(f"m.handle_id IN ({','.join('?' * len(handle_ids))})")
            params.extend(handle_ids)
        if chat is not None:
            clauses.append("(c.display_name = ? OR c.chat_identifier = ?)")
            params.extend([chat, chat])
        if from_me is not None:
            clauses.append("m.is_from_me = ?")
            params.append(int(from_me))
        if since is not None:
            clauses.append("m.date >= ?")
            params.append(_apple_ns(since))
        if until is not None:
            clauses.append("m.date <= ?")
            params.append(_apple_ns(until))
        where = ("WHERE " + " AND ".join(clauses)) if clauses else ""
        optional_cols = _optional_columns(con)
        sql = f"""
            SELECT m.ROWID AS rowid, m.guid AS guid, m.date AS date, m.text AS text,
                   m.attributedBody AS attributed_body,
                   m.is_from_me AS is_from_me, m.is_read AS is_read,
                   m.service AS service, h.id AS handle,
                   {optional_cols},
                   c.ROWID AS chat_id, c.display_name AS chat_name,
                   c.chat_identifier AS chat_identifier,
                   (SELECT COUNT(*) FROM message_attachment_join maj
                    WHERE maj.message_id = m.ROWID) AS n_attachments
            FROM message m
            LEFT JOIN handle h ON m.handle_id = h.ROWID
            LEFT JOIN chat_message_join cmj ON cmj.message_id = m.ROWID
            LEFT JOIN chat c ON c.ROWID = cmj.chat_id
            {where}
            ORDER BY m.date DESC
            LIMIT ?
        """
        rows = con.execute(sql, [*params, int(limit)]).fetchall()
        reply_to = _resolve_reply_targets(con, rows)
    finally:
        con.close()

    records = []
    name_index = _name_index(None) if resolve_names else {}
    for r in rows:
        (rowid, guid, date, t, ab, is_from_me, is_read, service, handle,
         reply_to_raw, assoc_guid, assoc_type, date_edited, date_retracted,
         chat_id, chat_name, chat_identifier, n_att) = r
        reply_to_guid = _bare_guid(reply_to_raw)
        target_rowid, target_text = reply_to.get(reply_to_guid, (None, None))
        records.append({
            "rowid": rowid,
            "guid": guid,
            "date": date,
            "name": name_index.get(_norm(handle)),
            "handle": handle,
            "is_from_me": bool(is_from_me),
            "text": t if t is not None else _decode_attributed_body(ab),
            "reply_to_guid": reply_to_guid,
            "reply_to_rowid": target_rowid,
            "reply_to_text": target_text,
            "tapback": _tapback(assoc_type),
            "tapback_target_guid": _bare_guid(assoc_guid),
            "edited": bool(date_edited),
            "unsent": bool(date_retracted),
            "service": service,
            "chat_id": chat_id,
            "chat_name": chat_name,
            "chat_identifier": chat_identifier,
            "is_read": bool(is_read),
            "n_attachments": n_att,
        })
    if not records:
        return _empty_messages()
    return pl.DataFrame(records, schema=_MESSAGE_SCHEMA).with_columns(_to_datetime("date").alias("date"))


def _resolve_reply_targets(
    con: sqlite3.Connection, rows: list[tuple]
) -> dict[str, tuple[int, str | None]]:
    """Map each replied-to GUID in ``rows`` to its ``(rowid, text)``.

    A threaded reply only stores the originator's GUID; one extra query resolves
    those GUIDs to the original message's row id and decoded text so a reply is
    readable on its own. ``reply_to_raw`` is the 10th selected column.
    """

    wanted = {_bare_guid(r[9]) for r in rows}
    wanted.discard(None)
    if not wanted:
        return {}
    placeholders = ",".join("?" * len(wanted))
    resolved: dict[str, tuple[int, str | None]] = {}
    for guid, rowid, text, ab in con.execute(
        f"SELECT guid, ROWID, text, attributedBody FROM message WHERE guid IN ({placeholders})",
        list(wanted),
    ):
        resolved[guid] = (rowid, text if text is not None else _decode_attributed_body(ab))
    return resolved


_MESSAGE_SCHEMA = {
    "rowid": pl.Int64, "guid": pl.Utf8, "date": pl.Int64, "name": pl.Utf8,
    "handle": pl.Utf8, "is_from_me": pl.Boolean, "text": pl.Utf8,
    "reply_to_guid": pl.Utf8, "reply_to_rowid": pl.Int64, "reply_to_text": pl.Utf8,
    "tapback": pl.Utf8, "tapback_target_guid": pl.Utf8,
    "edited": pl.Boolean, "unsent": pl.Boolean,
    "service": pl.Utf8, "chat_id": pl.Int64, "chat_name": pl.Utf8,
    "chat_identifier": pl.Utf8, "is_read": pl.Boolean, "n_attachments": pl.Int64,
}


def _empty_messages() -> pl.DataFrame:
    schema = dict(_MESSAGE_SCHEMA)
    schema["date"] = pl.Datetime("ns", "UTC")
    return pl.DataFrame(schema=schema)


def chats(*, limit: int = 200, db: str | None = None) -> pl.DataFrame:
    """Conversations as a polars DataFrame, most-recently-active first.

    Columns: ``chat_id``, ``chat_name`` (group display name or None),
    ``chat_identifier`` (the phone/email/group id), ``service``, ``n_messages``,
    ``last_date`` (UTC datetime of the latest message). Pass ``limit`` to cap the
    number of conversations returned.
    """

    con = _connect(db or CHAT_DB)
    try:
        rows = con.execute(
            """
            SELECT c.ROWID AS chat_id, c.display_name AS chat_name,
                   c.chat_identifier AS chat_identifier, c.service_name AS service,
                   COUNT(m.ROWID) AS n_messages, MAX(m.date) AS last_date
            FROM chat c
            JOIN chat_message_join cmj ON cmj.chat_id = c.ROWID
            JOIN message m ON m.ROWID = cmj.message_id
            GROUP BY c.ROWID
            ORDER BY last_date DESC
            LIMIT ?
            """,
            [int(limit)],
        ).fetchall()
    finally:
        con.close()

    schema = {
        "chat_id": pl.Int64, "chat_name": pl.Utf8, "chat_identifier": pl.Utf8,
        "service": pl.Utf8, "n_messages": pl.Int64, "last_date": pl.Int64,
    }
    if not rows:
        schema["last_date"] = pl.Datetime("ns", "UTC")
        return pl.DataFrame(schema=schema)
    df = pl.DataFrame(rows, schema=schema, orient="row")
    return df.with_columns(_to_datetime("last_date").alias("last_date"))


# AppleScript to send a message: takes the recipient and body as `run` arguments
# (never interpolated into the script text), so a message body cannot inject
# AppleScript. The service is a fixed allowlist value, so it is safe to format in.
_SEND_SCRIPT = """
on run {{targetRecipient, targetMessage}}
    tell application "Messages"
        set targetService to 1st account whose service type = {service}
        set targetBuddy to participant targetRecipient of targetService
        send targetMessage to targetBuddy
    end tell
end run
"""

_SERVICES = {"iMessage": "iMessage", "SMS": "SMS"}


def send(to: str, text: str, *, service: str = "iMessage", timeout: float = 30.0) -> None:
    """Send a message to ``to`` (a phone number or email) through the Messages app.

    ``service`` is ``"iMessage"`` (blue, the default) or ``"SMS"`` (green, only if
    your Mac has Text Message Forwarding from an iPhone). Drives the Messages app
    over AppleScript, which on first use prompts for Automation permission to
    control "Messages"; grant it and retry. Raises ``RuntimeError`` if the send
    fails (e.g. the recipient is unreachable on that service).
    """

    if service not in _SERVICES:
        raise ValueError(f"imessage.send: service must be one of {sorted(_SERVICES)}, not {service!r}.")
    script = _SEND_SCRIPT.format(service=_SERVICES[service])
    proc = subprocess.run(
        ["osascript", "-e", script, to, text],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or "unknown error"
        raise RuntimeError(
            f"imessage.send to {to!r} via {service} failed: {detail}. If this is "
            "an Automation-permission prompt, grant control of Messages under "
            "System Settings > Privacy & Security > Automation, then retry."
        )


# --- Contact editing -------------------------------------------------------
#
# Writes go through the Contacts app (JXA over osascript), never the SQLite
# store: contactsd owns that database, and an edit made through the app lands in
# the default account and syncs to iCloud exactly like a hand-made edit, while a
# direct SQLite write would be invisible to sync and risk corrupting the store.
# The whole operation is passed as ONE JSON `run` argument (never interpolated
# into the script text), so contact data cannot inject code. JXA rather than
# AppleScript because parsing structured input (lists of labeled phones/emails)
# is `JSON.parse` instead of string surgery.
_CONTACTS_SCRIPT = """
function run(argv) {
    const spec = JSON.parse(argv[0]);
    const app = Application("Contacts");
    const norm = (v) => {
        const s = String(v).trim();
        if (s.includes("@")) return s.toLowerCase();
        const digits = s.replace(/\\D/g, "");
        return digits.length >= 10 ? digits.slice(-10) : digits;
    };
    const FIELDS = { first: "firstName", last: "lastName", organization: "organization" };

    if (spec.op === "add") {
        const props = {};
        for (const [key, prop] of Object.entries(FIELDS))
            if (spec[key] != null) props[prop] = spec[key];
        const person = app.Person(props);
        app.people.push(person);
        for (const p of spec.addPhones) person.phones.push(app.Phone({ label: p.label, value: p.value }));
        for (const e of spec.addEmails) person.emails.push(app.Email({ label: e.label, value: e.value }));
        app.save();
        return person.id();
    }

    const person = app.people.byId(spec.id);
    person.id(); // touch it so a stale/foreign id fails here with a clear error

    if (spec.op === "delete") {
        app.delete(person);
        app.save();
        return "";
    }

    for (const [key, prop] of Object.entries(FIELDS))
        if (spec[key] != null) person[prop] = spec[key];
    const drop = (elements, wanted) => {
        const keys = new Set(wanted.map(norm));
        // Snapshot before deleting: removing while iterating a live elements
        // proxy skips neighbors.
        for (const el of Array.from(elements())) if (keys.has(norm(el.value()))) app.delete(el);
    };
    if (spec.removePhones.length) drop(person.phones, spec.removePhones);
    if (spec.removeEmails.length) drop(person.emails, spec.removeEmails);
    for (const p of spec.addPhones) person.phones.push(app.Phone({ label: p.label, value: p.value }));
    for (const e of spec.addEmails) person.emails.push(app.Email({ label: e.label, value: e.value }));
    app.save();
    return person.id();
}
"""

# Contacts stores its built-in labels in the `_$!<...>!$_` form; a label set to
# the plain word would show up as a custom label, so the common ones are mapped
# to the canonical spelling and anything else passes through as a custom label.
_LABELS = {
    name.lower(): f"_$!<{name}>!$_"
    for name in ("Mobile", "iPhone", "Home", "Work", "Main", "School", "Other", "HomePage")
}


def _labeled(values: Iterable[str | tuple[str, str]] | None, default_label: str) -> list[dict[str, str]]:
    """Normalize ``["+1..."]`` / ``[("work", "+1...")]`` to JXA label/value dicts."""

    out = []
    for value in values or ():
        label, value = value if isinstance(value, tuple) else (default_label, value)
        out.append({"label": _LABELS.get(label.lower(), label), "value": value})
    return out


def _contact_id(contact: str) -> str:
    """Resolve a contact argument (unique id or name) to an address-book id.

    An id (``"...:ABPerson"``) passes through; anything else is matched against
    contact names, case-insensitive substring, and must hit exactly one person.
    """

    if contact.endswith(":ABPerson"):
        return contact
    needle = contact.strip().lower()
    matches = [
        (row["id"], row["name"])
        for row in contacts().iter_rows(named=True)
        if needle in (row["name"] or "").lower()
    ]
    if len(matches) == 1:
        return matches[0][0]
    if not matches:
        raise LookupError(f"imessage: no contact matching {contact!r}.")
    names = ", ".join(f"{name} ({cid})" for cid, name in matches[:10])
    raise LookupError(
        f"imessage: {contact!r} matches {len(matches)} contacts: {names}. "
        "Pass the id instead."
    )


def _run_contacts_script(spec: dict, timeout: float) -> str:
    proc = subprocess.run(
        ["osascript", "-l", "JavaScript", "-e", _CONTACTS_SCRIPT, json.dumps(spec)],
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or "unknown error"
        raise RuntimeError(
            f"imessage: contact {spec['op']} failed: {detail}. If this is an "
            "Automation-permission prompt, grant control of Contacts under "
            "System Settings > Privacy & Security > Automation, then retry."
        )
    return proc.stdout.strip()


def add_contact(
    *,
    first: str | None = None,
    last: str | None = None,
    organization: str | None = None,
    phones: Iterable[str | tuple[str, str]] | None = None,
    emails: Iterable[str | tuple[str, str]] | None = None,
    timeout: float = 30.0,
) -> str:
    """Create a contact and return its address-book id.

    ``phones`` and ``emails`` are plain strings (labeled ``mobile`` / ``home``)
    or ``(label, value)`` tuples; common labels (``mobile``, ``work``, ...)
    map to the Contacts built-ins, anything else becomes a custom label. Saves
    through the Contacts app into the default account, so on an iCloud-signed-in
    Mac the new contact syncs everywhere (see the module docstring for the
    Automation permission).
    """

    if not any((first, last, organization)):
        raise ValueError("imessage.add_contact: pass at least one of first/last/organization.")
    return _run_contacts_script(
        {
            "op": "add",
            "first": first, "last": last, "organization": organization,
            "addPhones": _labeled(phones, "mobile"),
            "addEmails": _labeled(emails, "home"),
        },
        timeout,
    )


def update_contact(
    contact: str,
    *,
    first: str | None = None,
    last: str | None = None,
    organization: str | None = None,
    add_phones: Iterable[str | tuple[str, str]] | None = None,
    add_emails: Iterable[str | tuple[str, str]] | None = None,
    remove_phones: Iterable[str] | None = None,
    remove_emails: Iterable[str] | None = None,
    timeout: float = 30.0,
) -> str:
    """Edit a contact in place and return its address-book id.

    ``contact`` is an id from :func:`contacts` (``"...:ABPerson"``) or a name,
    which must match exactly one person. ``first`` / ``last`` /
    ``organization`` replace the field when given (``""`` clears it, ``None``
    leaves it alone). ``add_phones`` / ``add_emails`` take the same values as
    :func:`add_contact`; ``remove_phones`` / ``remove_emails`` match loosely
    (phones on their last 10 digits, emails case-insensitively), so
    ``"+1 (202) 555-0123"`` removes ``2025550123``. Removals run before
    additions, so replacing a number is one call. Saves through the Contacts
    app, so the edit syncs to iCloud like a hand-made one.
    """

    spec = {
        "op": "update",
        "id": _contact_id(contact),
        "first": first, "last": last, "organization": organization,
        "addPhones": _labeled(add_phones, "mobile"),
        "addEmails": _labeled(add_emails, "home"),
        "removePhones": list(remove_phones or ()),
        "removeEmails": list(remove_emails or ()),
    }
    if not (
        any(v is not None for v in (first, last, organization))
        or spec["addPhones"] or spec["addEmails"] or spec["removePhones"] or spec["removeEmails"]
    ):
        raise ValueError("imessage.update_contact: nothing to change.")
    return _run_contacts_script(spec, timeout)


def delete_contact(contact: str, *, timeout: float = 30.0) -> None:
    """Delete a contact by id or unique name (resolved like :func:`update_contact`).

    Deletes through the Contacts app, so the removal syncs to iCloud.
    """

    _run_contacts_script({"op": "delete", "id": _contact_id(contact)}, timeout)
