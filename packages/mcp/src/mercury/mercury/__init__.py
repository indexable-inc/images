"""Mercury bank for the kernel: read accounts and transactions, attach receipts.

Mercury exposes a public REST API (base ``https://api.mercury.com/api/v1``,
Bearer-token auth) so a session can ``import mercury`` with no install step and
pull bank data as polars frames.

    import mercury

    mercury.login("secret-token:mercury_production_...")   # store your token (mode 0600)
    await mercury.status()                                 # {"configured": True, ...}
    mercury.logout()                                       # remove the stored token

    await mercury.accounts()                               # bank accounts, as a polars frame
    await mercury.transactions(limit=50)                   # transactions, newest first
    await mercury.transactions(account_id="...", status="sent")
    await mercury.transaction("<tx id>", account_id="...") # one transaction
    await mercury.attach_receipt("<tx id>", "receipt.pdf") # upload a file attachment

Each read call returns a polars DataFrame with a fixed schema so empty results
stay typed. Credentials are per-user and never shared: the API token is read from
the ``MERCURY_API_TOKEN`` (or ``MERCURY_TOKEN``) environment variable, or from a
user-only file at ``~/.config/mercury/token`` (written mode 0600 by
:func:`login`). No token is baked into the repo. Mint one in the Mercury
dashboard under Settings -> API Tokens; it includes the ``secret-token:`` prefix.
In a shared (multiplayer) room (``IX_MCP_SHARED`` set) every data call raises
before the token is read or any network request is made, so bank data never
reaches state other participants can see.

Raises :exc:`MercuryError` when no token is configured (the message names
``mercury.login(token)``) or when the API cannot be reached.
"""

from __future__ import annotations

import asyncio
import json
import os
import pathlib
from typing import Any

import httpx
import polars as pl
from pydantic import BaseModel, ConfigDict, Field

__all__ = [
    "MercuryError",
    "account",
    "accounts",
    "all_transactions",
    "attach_receipt",
    "card",
    "cards",
    "categories",
    "credit",
    "customer",
    "customers",
    "event",
    "events",
    "invoice",
    "invoices",
    "login",
    "logout",
    "organization",
    "recipient",
    "recipient_attachments",
    "recipients",
    "safes",
    "send_money_approval_requests",
    "statements",
    "status",
    "transaction",
    "transactions",
    "treasury",
    "treasury_statements",
    "treasury_transactions",
    "user",
    "users",
    "webhooks",
]

__version__ = "0.1.0"

# Environment variables checked for a token, in resolution order. MERCURY_API_TOKEN
# is the primary name; MERCURY_TOKEN is accepted as a common alias.
_TOKEN_ENV_VARS = ("MERCURY_API_TOKEN", "MERCURY_TOKEN")

# A shared (multiplayer) room marks the MCP it replicates across participants
# with this env var. Mercury reads bank balances and transaction history with a
# per-user token, so -- like the other personal-credential modules here -- it is
# confined to incognito sessions; only a truthy value refuses access.
SHARED_ENV = "IX_MCP_SHARED"

# The per-user token file path (mode 0600).
_TOKEN_FILE = pathlib.Path.home() / ".config" / "mercury" / "token"

# Mercury REST API base URL; overridable for a sandbox/proxy endpoint.
_BASE_URL_ENV = "MERCURY_API_BASE_URL"
_DEFAULT_BASE_URL = "https://api.mercury.com/api/v1"

# Per-request timeout (seconds).
_TIMEOUT = 30.0

# Timestamp columns are parsed from the API's ISO 8601 strings into a real
# tz-aware Datetime so callers can do native polars time math (filter by date,
# sort) instead of string compares.
_TS = pl.Datetime(time_unit="us", time_zone="UTC")

# Fixed schemas so empty results stay typed.
_ACCOUNTS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "name": pl.Utf8,
    "nickname": pl.Utf8,
    "type": pl.Utf8,
    "kind": pl.Utf8,
    "status": pl.Utf8,
    "account_number": pl.Utf8,
    "routing_number": pl.Utf8,
    "available_balance": pl.Float64,
    "current_balance": pl.Float64,
    "legal_business_name": pl.Utf8,
    "created_at": _TS,
    "dashboard_link": pl.Utf8,
}

_TRANSACTIONS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    # Signed: negative for a debit (money out), positive for a credit (money in).
    "amount": pl.Float64,
    "status": pl.Utf8,
    "counterparty": pl.Utf8,
    "bank_description": pl.Utf8,
    "kind": pl.Utf8,
    "note": pl.Utf8,
    "external_memo": pl.Utf8,
    "created_at": _TS,
    "posted_at": _TS,
    "attachments": pl.Int64,
    "dashboard_link": pl.Utf8,
}


# Pydantic models for the API response objects: validate-and-default the raw JSON
# (``extra="ignore"`` drops fields we don't surface) so the puller bodies read
# typed fields instead of fragile ``dict.get`` chains. Timestamps stay ``str``
# here so :func:`_frame` remains the single place datetimes are parsed.
class _ApiModel(BaseModel):
    model_config = ConfigDict(extra="ignore")


class _Account(_ApiModel):
    id: str = ""
    name: str = ""
    nickname: str | None = None
    type: str = ""
    kind: str = ""
    status: str = ""
    account_number: str = Field("", validation_alias="accountNumber")
    routing_number: str = Field("", validation_alias="routingNumber")
    available_balance: float | None = Field(None, validation_alias="availableBalance")
    current_balance: float | None = Field(None, validation_alias="currentBalance")
    legal_business_name: str = Field("", validation_alias="legalBusinessName")
    created_at: str = Field("", validation_alias="createdAt")
    dashboard_link: str = Field("", validation_alias="dashboardLink")


class _Transaction(_ApiModel):
    id: str = ""
    amount: float | None = None
    status: str = ""
    counterparty_name: str = Field("", validation_alias="counterpartyName")
    bank_description: str | None = Field(None, validation_alias="bankDescription")
    kind: str = ""
    note: str | None = None
    external_memo: str | None = Field(None, validation_alias="externalMemo")
    created_at: str = Field("", validation_alias="createdAt")
    posted_at: str | None = Field(None, validation_alias="postedAt")
    attachments: list[dict[str, Any]] = Field(default_factory=list)
    dashboard_link: str = Field("", validation_alias="dashboardLink")


class _AccountsResponse(_ApiModel):
    accounts: list[_Account] = Field(default_factory=list)


class _TransactionsResponse(_ApiModel):
    total: int = 0
    transactions: list[_Transaction] = Field(default_factory=list)


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
    out, and ``strict=False`` only nulls *individual* failures, not that.
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


class MercuryError(RuntimeError):
    """Raised when the Mercury API cannot be reached for this session.

    Usually means "not configured": call ``mercury.login(token)`` to store an
    API token (minted in the Mercury dashboard under Settings -> API Tokens).
    Also raised on HTTP errors from the Mercury REST API (a rejected token
    surfaces as a 401/403 naming the next step).
    """


def _require_incognito() -> None:
    """Refuse to access Mercury data in a shared (multiplayer) room.

    Mercury calls return bank balances, transaction history, and account/routing
    numbers, so a shared room would leak one person's finances into state
    everyone can see. A shared room sets ``IX_MCP_SHARED``; only then is access
    refused -- before the token is read or any request is sent.
    """
    if os.environ.get(SHARED_ENV):
        raise MercuryError(
            "Mercury is not available in a shared (multiplayer) room "
            "(IX_MCP_SHARED is set), because it would expose personal bank "
            "accounts and transactions to everyone in the room. Use it from an "
            "incognito chat instead; its transcript stays private to you."
        )


def _base_url() -> str:
    """The Mercury REST API base URL (no trailing slash)."""
    val = os.environ.get(_BASE_URL_ENV, "").strip()
    return (val or _DEFAULT_BASE_URL).rstrip("/")


def _token() -> str:
    """Return the Mercury API token, or raise MercuryError if none is configured.

    Resolution order: ``MERCURY_API_TOKEN`` env, ``MERCURY_TOKEN`` env, then
    ``~/.config/mercury/token`` (written by :func:`login`).
    """
    for var in _TOKEN_ENV_VARS:
        val = os.environ.get(var, "").strip()
        if val:
            return val
    if _TOKEN_FILE.exists():
        val = _TOKEN_FILE.read_text().strip()
        if val:
            return val
    raise MercuryError(
        "No Mercury API token is configured for this session. "
        "Call `mercury.login(token)` with a token minted in the Mercury "
        "dashboard (Settings -> API Tokens; it starts with `secret-token:`), "
        "set the MERCURY_API_TOKEN environment variable, or run "
        "`mercury.status()` to check the current state."
    )


async def _request(
    method: str,
    path: str,
    *,
    params: dict[str, Any] | None = None,
    files: dict[str, Any] | None = None,
    data: dict[str, Any] | None = None,
) -> httpx.Response:
    """Call the Mercury REST API and return the response, or raise MercuryError.

    The token goes in an ``Authorization: Bearer`` header (never the URL), so it
    stays out of logs. Refused outright in a shared (multiplayer) room, before
    the token is even read. Raises :exc:`MercuryError` on a transport failure or
    an HTTP error status; a 401/403 names the re-login step.
    """
    _require_incognito()
    token = _token()
    url = f"{_base_url()}{path}"
    try:
        async with httpx.AsyncClient(timeout=_TIMEOUT) as client:
            resp = await client.request(
                method,
                url,
                params=params,
                files=files,
                data=data,
                headers={"Authorization": f"Bearer {token}"},
            )
    except httpx.HTTPError as exc:
        raise MercuryError(f"Mercury API request failed for {path}: {exc}") from exc

    if resp.status_code in (401, 403):
        raise MercuryError(
            f"Mercury API token was rejected (HTTP {resp.status_code}). "
            "Call `mercury.login(token)` with a fresh token from the Mercury "
            "dashboard (Settings -> API Tokens)."
        )
    if resp.status_code >= 400:
        raise MercuryError(
            f"Mercury API error for {path}: HTTP {resp.status_code} {resp.text[:200]}"
        )
    return resp


def login(token: str) -> dict[str, Any]:
    """Store a Mercury API token for this user.

    Writes ``token`` to ``~/.config/mercury/token`` with mode 0600 so only this
    user can read it. Mint the token in the Mercury dashboard under
    Settings -> API Tokens (it starts with ``secret-token:``). Returns
    ``{"configured": True, "path": str}``.

    Call ``mercury.status()`` afterwards to confirm the token is valid.
    """
    token = token.strip()
    if not token:
        raise MercuryError("token must not be empty")
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
    """Remove the stored Mercury token file.

    Idempotent: returns ``{"signed_out": True, "removed": bool}`` whether or not
    the file existed. Does not revoke the token at Mercury.
    """
    removed = _TOKEN_FILE.exists()
    _TOKEN_FILE.unlink(missing_ok=True)
    return {"signed_out": True, "removed": removed}


async def status() -> dict[str, Any]:
    """Whether this session has a Mercury token configured, and as whom.

    Returns ``{"configured": bool, "base_url": str, "accounts": int | None}`` and
    never raises: a missing or rejected token is reported as ``configured=False``,
    not an exception. Probes ``GET /accounts`` (never printing any secret).
    Call ``mercury.login(token)`` to configure.
    """
    base = _base_url()
    try:
        resp = await _request("GET", "/accounts")
    except MercuryError:
        return {"configured": False, "base_url": base, "accounts": None}
    parsed = _AccountsResponse.model_validate(resp.json())
    return {"configured": True, "base_url": base, "accounts": len(parsed.accounts)}


async def accounts() -> pl.DataFrame:
    """Mercury bank accounts, as a polars DataFrame.

    Columns: ``id``, ``name``, ``nickname``, ``type`` (``"mercury"`` /
    ``"external"`` / ``"recipient"``), ``kind``, ``status``, ``account_number``,
    ``routing_number``, ``available_balance``, ``current_balance``,
    ``legal_business_name``, ``created_at`` (tz-aware UTC datetime),
    ``dashboard_link``.

    Use ``id`` to scope :func:`transactions`. Raises :exc:`MercuryError` when no
    token is configured or the API is unreachable.
    """
    resp = await _request("GET", "/accounts")
    parsed = _AccountsResponse.model_validate(resp.json())
    rows: list[dict[str, Any]] = [
        {
            "id": a.id,
            "name": a.name,
            "nickname": a.nickname or "",
            "type": a.type,
            "kind": a.kind,
            "status": a.status,
            "account_number": a.account_number,
            "routing_number": a.routing_number,
            "available_balance": a.available_balance,
            "current_balance": a.current_balance,
            "legal_business_name": a.legal_business_name,
            "created_at": a.created_at,
            "dashboard_link": a.dashboard_link,
        }
        for a in parsed.accounts
    ]
    return _frame(rows, _ACCOUNTS_SCHEMA)


async def _first_account_id() -> str:
    """The id of the first Mercury (deposit) account, for the default scope.

    Mercury's transaction endpoint is per-account; when the caller does not name
    one, default to the first ``type="mercury"`` account (falling back to the
    first account of any type). Raises :exc:`MercuryError` if there are none.
    """
    resp = await _request("GET", "/accounts")
    parsed = _AccountsResponse.model_validate(resp.json())
    if not parsed.accounts:
        raise MercuryError(
            "No Mercury accounts are visible to this token. Check the token's "
            "scope in the Mercury dashboard (Settings -> API Tokens)."
        )
    mercury_accounts = [a for a in parsed.accounts if a.type == "mercury"]
    return (mercury_accounts or parsed.accounts)[0].id


# Keys whose value looks like an ISO 8601 timestamp across Mercury resources.
# The generic frame builder (:func:`_records_frame`) parses any column whose name
# ends with one of these into a tz-aware UTC datetime, so callers get native time
# math on every resource, not just the hand-schemaed core ones.
_TIME_KEY_SUFFIXES = ("At", "_at", "Date", "_date", "Time", "_time")


def _is_time_key(name: str) -> bool:
    return name.endswith(_TIME_KEY_SUFFIXES)


def _records_frame(records: list[dict[str, Any]]) -> pl.DataFrame:
    """Turn a list of JSON objects into a polars DataFrame, scalars-only.

    Used for resources without a hand-written schema. Each object's scalar fields
    (str/int/float/bool/None) become columns; nested objects and arrays are
    JSON-encoded into a string column so the frame stays flat and never raises on
    a struct-vs-string dtype clash across rows. Any column whose name reads like a
    timestamp (see :data:`_TIME_KEY_SUFFIXES`) is parsed to a tz-aware UTC
    datetime. An empty input returns an empty (column-less) frame, which still
    composes under polars.

    This keeps the contract "every Mercury resource is reachable as a polars
    DataFrame" without a bespoke schema for each of a dozen rarely-touched
    endpoints, while the core resources (accounts, transactions, ...) keep their
    fixed, typed schemas above.
    """
    if not records:
        return pl.DataFrame()
    # Union of keys across records so a field missing from one row still appears
    # as a (null-filled) column, and the column order is stable (first-seen).
    keys: list[str] = []
    seen: set[str] = set()
    for rec in records:
        for k in rec:
            if k not in seen:
                seen.add(k)
                keys.append(k)
    rows: list[dict[str, Any]] = []
    for rec in records:
        row: dict[str, Any] = {}
        for k in keys:
            v = rec.get(k)
            # Flatten nested objects/arrays to JSON text so a column never mixes a
            # struct dtype in one row with null/str in another (which polars would
            # reject); scalars pass through for native dtypes and time parsing.
            row[k] = json.dumps(v, default=str) if isinstance(v, (dict, list)) else v
        rows.append(row)
    df = pl.DataFrame(rows, infer_schema_length=None)
    time_cols = [c for c in df.columns if _is_time_key(c) and df.schema[c] == pl.Utf8]
    for c in time_cols:
        # Mirror _frame: with no parseable sample (every value empty/missing),
        # polars' format inference raises ComputeError even under strict=False,
        # so emit a typed null column instead of calling str.to_datetime.
        has_value = bool(
            (df.get_column(c).str.strip_chars().str.len_chars().fill_null(0) > 0).any()
        )
        if has_value:
            df = df.with_columns(
                pl.col(c).str.to_datetime(time_zone="UTC", strict=False).alias(c)
            )
        else:
            df = df.with_columns(pl.lit(None, dtype=_TS).alias(c))
    return df


def _envelope_items(body: object, *keys: str) -> list[dict[str, Any]]:
    """Pull the list of items out of a Mercury list response.

    Mercury list endpoints return either a bare JSON array or an object wrapping
    the array under a resource key (``{"accounts": [...]}``, ``{"recipients":
    [...]}``, ...). Return the first matching list (the named keys, then a generic
    ``items``/``data``), or ``[]`` when none is present.
    """
    if isinstance(body, list):
        return [x for x in body if isinstance(x, dict)]
    if isinstance(body, dict):
        for key in (*keys, "items", "data"):
            val = body.get(key)
            if isinstance(val, list):
                return [x for x in val if isinstance(x, dict)]
    return []


async def _list_frame(path: str, *keys: str, params: dict[str, Any] | None = None) -> pl.DataFrame:
    """GET a list endpoint and return its items as a generic polars DataFrame."""
    resp = await _request("GET", path, params=params)
    return _records_frame(_envelope_items(resp.json(), *keys))


def _tx_row(t: _Transaction) -> dict[str, Any]:
    """Project one transaction into the public row shape."""
    return {
        "id": t.id,
        "amount": t.amount,
        "status": t.status,
        "counterparty": t.counterparty_name,
        "bank_description": t.bank_description or "",
        "kind": t.kind,
        "note": t.note or "",
        "external_memo": t.external_memo or "",
        "created_at": t.created_at,
        "posted_at": t.posted_at or "",
        "attachments": len(t.attachments),
        "dashboard_link": t.dashboard_link,
    }


async def transactions(
    *,
    account_id: str | None = None,
    limit: int = 100,
    status: str | None = None,
    search: str | None = None,
    start: str | None = None,
    end: str | None = None,
    newest_first: bool = True,
) -> pl.DataFrame:
    """Transactions for one Mercury account, newest first, as a polars DataFrame.

    Columns: ``id``, ``amount`` (signed: negative is a debit/money out, positive
    is a credit/money in), ``status`` (``"pending"`` / ``"sent"`` / ``"failed"``
    / ...), ``counterparty``, ``bank_description``, ``kind``, ``note``,
    ``external_memo``, ``created_at`` (tz-aware UTC datetime), ``posted_at``
    (datetime, null while pending), ``attachments`` (count), ``dashboard_link``.

    ``account_id`` defaults to the first Mercury deposit account (see
    :func:`accounts`). ``limit`` caps the rows returned (the API allows 1..1000).
    Narrow with ``status`` (one of ``pending``/``sent``/``cancelled``/``failed``/
    ``reversed``/``blocked``), ``search`` (matches description or counterparty),
    and ``start`` / ``end`` (``YYYY-MM-DD`` or ISO 8601; the API defaults to the
    last 30 days when both are omitted).

    The frame is sorted newest-first by default (the order most callers want);
    pass ``newest_first=False`` for oldest-first. (The API's own ``order``
    parameter is honored under the hood, but we re-sort the materialized frame so
    the order is stable regardless of how the API paginates.) Raises
    :exc:`MercuryError` when no token is configured or the API is unreachable.
    """
    acct = account_id or await _first_account_id()
    params: dict[str, Any] = {
        "limit": max(1, min(limit, 1000)),
        "order": "desc" if newest_first else "asc",
    }
    if status:
        params["status"] = status
    if search:
        params["search"] = search
    if start:
        params["start"] = start
    if end:
        params["end"] = end

    resp = await _request("GET", f"/account/{acct}/transactions", params=params)
    parsed = _TransactionsResponse.model_validate(resp.json())
    rows = [_tx_row(t) for t in parsed.transactions]
    frame = _frame(rows, _TRANSACTIONS_SCHEMA)
    if frame.height:
        frame = frame.sort("created_at", descending=newest_first)
    return frame


async def transaction(id: str, *, account_id: str | None = None) -> pl.DataFrame:
    """One Mercury transaction by id, as a single-row polars DataFrame.

    Same columns as :func:`transactions`. Without ``account_id`` this uses the
    org-wide ``GET /transaction/{id}`` lookup, so the id is found no matter which
    account it belongs to; pass ``account_id`` to scope the lookup to one account
    (``GET /account/{account_id}/transaction/{id}``). Raises :exc:`MercuryError`
    when no token is configured, the id is not found, or the API is unreachable.
    """
    if account_id:
        resp = await _request("GET", f"/account/{account_id}/transaction/{id}")
    else:
        resp = await _request("GET", f"/transaction/{id}")
    parsed = _Transaction.model_validate(resp.json())
    return _frame([_tx_row(parsed)], _TRANSACTIONS_SCHEMA)


async def attach_receipt(
    transaction_id: str,
    file: str | os.PathLike[str],
    *,
    attachment_type: str = "receipt",
) -> dict[str, Any]:
    """Upload a file attachment to a Mercury transaction.

    ``transaction_id`` is the transaction's id (from :func:`transactions`).
    ``file`` is a path to the file to upload (a PDF or image receipt, max 32 MB).
    ``attachment_type`` is one of ``"receipt"``, ``"bill"``, or ``"other"``
    (default ``"receipt"``).

    Returns the API's response (typically ``{"attachmentId": ..., "downloadUrl":
    ...}``). Raises :exc:`MercuryError` when no token is configured, the
    transaction is not found, the file is too large or an unsupported type, or the
    API is unreachable.
    """
    if attachment_type not in ("receipt", "bill", "other"):
        raise MercuryError(
            f"attachment_type must be 'receipt', 'bill', or 'other', not {attachment_type!r}"
        )
    # Read the file off the event loop: a blocking open/read on the kernel's one
    # asyncio loop would freeze every other job, so the synchronous stat+read runs
    # in a worker thread and only the bytes cross back. (This also keeps the async
    # function free of the blocking-IO lints the package gates on.)
    name, content = await asyncio.to_thread(_read_upload, file)
    resp = await _request(
        "POST",
        f"/transaction/{transaction_id}/attachments",
        files={"file": (name, content)},
        data={"attachmentType": attachment_type},
    )
    body: dict[str, Any] = resp.json()
    return body


def _read_upload(file: str | os.PathLike[str]) -> tuple[str, bytes]:
    """Read an attachment file synchronously, returning ``(name, bytes)``.

    Kept separate (and called via ``asyncio.to_thread``) so the blocking stat and
    read never run on the kernel's event loop. Raises :exc:`MercuryError` if the
    path is not a regular file.
    """
    path = pathlib.Path(file)
    if not path.is_file():
        raise MercuryError(f"attachment file not found: {path}")
    return path.name, path.read_bytes()


# ---------------------------------------------------------------------------
# The rest of the Mercury read surface, every resource as a polars DataFrame.
#
# These use the generic record-to-frame builder (no bespoke schema): each list
# endpoint returns a flat polars DataFrame with the API's own field names as
# columns and timestamp-looking columns parsed to UTC datetimes. A single-item
# `get` returns the raw object as a dict (one nested record reads better as a
# dict than as a one-row frame of JSON-encoded sub-objects). The core resources
# above (accounts, transactions) keep their fixed, typed schemas.
# ---------------------------------------------------------------------------


async def account(id: str) -> dict[str, Any]:
    """One Mercury account by id, as a dict (``GET /account/{id}``)."""
    resp = await _request("GET", f"/account/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def all_transactions(
    *,
    limit: int = 100,
    status: str | None = None,
    start: str | None = None,
    end: str | None = None,
    newest_first: bool = True,
) -> pl.DataFrame:
    """Transactions across ALL accounts, as a polars DataFrame (``GET /transactions``).

    Where :func:`transactions` is scoped to one account, this is the
    organization-wide feed. Generic columns (the API's own field names);
    timestamp columns are parsed to UTC datetimes. ``limit`` caps the rows;
    narrow with ``status`` and ``start`` / ``end`` (``YYYY-MM-DD`` or ISO 8601).
    Newest first by default, matching :func:`transactions` (the API's own default
    is oldest-first, which would silently page in stale history); pass
    ``newest_first=False`` for oldest-first.
    """
    params: dict[str, Any] = {
        "limit": max(1, min(limit, 1000)),
        "order": "desc" if newest_first else "asc",
    }
    if status:
        params["status"] = status
    if start:
        params["start"] = start
    if end:
        params["end"] = end
    return await _list_frame("/transactions", "transactions", params=params)


async def cards(account_id: str | None = None) -> pl.DataFrame:
    """Debit/credit cards for an account, as a polars DataFrame (``GET /account/{id}/cards``).

    ``account_id`` defaults to the first Mercury deposit account (see
    :func:`accounts`).
    """
    acct = account_id or await _first_account_id()
    return await _list_frame(f"/account/{acct}/cards", "cards")


# A single card is read by scanning cards (Mercury has no get-card-by-id
# endpoint), so `card` is a filter over `cards`, returned as a 1-row frame for
# the matched id (empty frame if not found). Without an account_id every
# account's cards are searched, since the cards API is per-account and the card
# may hang off any of them.
async def card(card_id: str, *, account_id: str | None = None) -> pl.DataFrame:
    """One card by id as a 1-row polars DataFrame (filtered from :func:`cards`).

    ``account_id`` scopes the search to one account; when omitted, all accounts
    are searched in order.
    """
    if account_id:
        frame = await cards(account_id=account_id)
        if frame.height and "id" in frame.columns:
            return frame.filter(pl.col("id") == card_id)
        return frame.clear()
    resp = await _request("GET", "/accounts")
    parsed = _AccountsResponse.model_validate(resp.json())
    last = pl.DataFrame()
    for a in parsed.accounts:
        frame = await cards(account_id=a.id)
        if frame.height and "id" in frame.columns:
            match = frame.filter(pl.col("id") == card_id)
            if match.height:
                return match
            last = frame
    return last.clear()


async def statements(account_id: str | None = None) -> pl.DataFrame:
    """Monthly statements for an account, as a polars DataFrame (``GET /account/{id}/statements``).

    ``account_id`` defaults to the first Mercury deposit account.
    """
    acct = account_id or await _first_account_id()
    return await _list_frame(f"/account/{acct}/statements", "statements")


async def recipients(*, limit: int = 500) -> pl.DataFrame:
    """Payment recipients, as a polars DataFrame (``GET /recipients``)."""
    return await _list_frame(
        "/recipients", "recipients", params={"limit": max(1, min(limit, 1000))}
    )


async def recipient(id: str) -> dict[str, Any]:
    """One recipient by id, as a dict (``GET /recipient/{id}``)."""
    resp = await _request("GET", f"/recipient/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def recipient_attachments(*, limit: int = 500) -> pl.DataFrame:
    """All recipient tax-form attachments, as a polars DataFrame (``GET /recipients/attachments``)."""
    return await _list_frame(
        "/recipients/attachments", "attachments", params={"limit": max(1, min(limit, 1000))}
    )


async def categories() -> pl.DataFrame:
    """Custom expense categories, as a polars DataFrame (``GET /categories``)."""
    return await _list_frame("/categories", "categories")


async def credit() -> pl.DataFrame:
    """Credit accounts for the organization, as a polars DataFrame (``GET /credit``)."""
    return await _list_frame("/credit", "credit", "creditAccounts", "accounts")


async def treasury() -> pl.DataFrame:
    """Treasury (investment) accounts, as a polars DataFrame (``GET /treasury``)."""
    return await _list_frame("/treasury", "treasury", "accounts")


async def treasury_transactions(treasury_id: str, *, limit: int = 100) -> pl.DataFrame:
    """Transactions for one treasury account, as a polars DataFrame.

    ``GET /treasury/{id}/transactions``; ``limit`` caps the rows returned.
    """
    return await _list_frame(
        f"/treasury/{treasury_id}/transactions",
        "transactions",
        params={"limit": max(1, min(limit, 1000))},
    )


async def treasury_statements(treasury_id: str) -> pl.DataFrame:
    """Statements for one treasury account, as a polars DataFrame (``GET /treasury/{id}/statements``)."""
    return await _list_frame(f"/treasury/{treasury_id}/statements", "statements")


async def users() -> pl.DataFrame:
    """Organization users, as a polars DataFrame (``GET /users``)."""
    return await _list_frame("/users", "users")


async def user(id: str) -> dict[str, Any]:
    """One user by id, as a dict (``GET /users/{id}``)."""
    resp = await _request("GET", f"/users/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def organization() -> dict[str, Any]:
    """Organization information (EIN, legal name, DBAs), as a dict (``GET /organization``)."""
    resp = await _request("GET", "/organization")
    body: dict[str, Any] = resp.json()
    return body


async def events(*, limit: int = 100) -> pl.DataFrame:
    """The organization's auditable event stream, as a polars DataFrame (``GET /events``).

    ``limit`` caps the rows returned. Timestamp columns are parsed to UTC
    datetimes; before/after value payloads come back as JSON-text columns.
    """
    return await _list_frame("/events", "events", params={"limit": max(1, min(limit, 1000))})


async def event(id: str) -> dict[str, Any]:
    """One event by id, as a dict (``GET /events/{id}``)."""
    resp = await _request("GET", f"/events/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def customers(*, limit: int = 500) -> pl.DataFrame:
    """Accounts-receivable customers, as a polars DataFrame (``GET /ar/customers``)."""
    return await _list_frame(
        "/ar/customers", "customers", params={"limit": max(1, min(limit, 1000))}
    )


async def customer(id: str) -> dict[str, Any]:
    """One customer by id, as a dict (``GET /ar/customers/{id}``)."""
    resp = await _request("GET", f"/ar/customers/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def invoices(*, limit: int = 500) -> pl.DataFrame:
    """Accounts-receivable invoices, as a polars DataFrame (``GET /ar/invoices``)."""
    return await _list_frame(
        "/ar/invoices", "invoices", params={"limit": max(1, min(limit, 1000))}
    )


async def invoice(id: str) -> dict[str, Any]:
    """One invoice by id, as a dict (``GET /ar/invoices/{id}``)."""
    resp = await _request("GET", f"/ar/invoices/{id}")
    body: dict[str, Any] = resp.json()
    return body


async def safes() -> pl.DataFrame:
    """SAFE (Simple Agreement for Future Equity) requests, as a polars DataFrame (``GET /safes``)."""
    return await _list_frame("/safes", "safes", "requests")


async def send_money_approval_requests(
    *,
    account_id: str | None = None,
    status: str | None = None,
) -> pl.DataFrame:
    """Send-money approval requests, as a polars DataFrame.

    ``GET /request-send-money``; narrow with ``account_id`` and ``status``.
    """
    params: dict[str, Any] = {}
    if account_id:
        params["accountId"] = account_id
    if status:
        params["status"] = status
    return await _list_frame(
        "/request-send-money",
        "requests",
        "approvalRequests",
        params=params or None,
    )


async def webhooks() -> pl.DataFrame:
    """Configured webhook endpoints, as a polars DataFrame (``GET /webhooks``)."""
    return await _list_frame("/webhooks", "webhooks", "endpoints")
