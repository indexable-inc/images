"""Notion workspace over the REST API using NOTION_API_KEY.

Bundled like ``linear``/``slack`` so every session can ``import notion`` and
read or write Notion pages, databases, and blocks without hand-rolling an httpx
client each time. When ``NOTION_API_KEY`` is present in the environment the
operations agents actually need work immediately with no additional setup:

    import notion

    # Search pages and databases (returns a polars DataFrame)
    df = await notion.search("roadmap")
    df = await notion.search("", filter="database")     # databases only

    # Read a page (returns a typed `Page` model -- attribute access)
    page = await notion.page("<page_id>")
    page.url                       # the page's Notion URL
    page.properties["Name"]        # raw property value

    # Read a page's / block's children (a polars DataFrame, paginated)
    blocks = await notion.blocks("<block_id>")

    # Query a database (a polars DataFrame, paginated)
    rows = await notion.db_query("<database_id>")
    rows = await notion.db_query("<database_id>", sorts=[
        {"property": "Name", "direction": "ascending"},
    ])

    # Create a page under a parent database or page
    new = await notion.page_create(
        {"database_id": "<database_id>"},
        {"Name": {"title": [{"text": {"content": "New row"}}]}},
    )
    new.id                         # the new page's id

    # Append blocks to a page / block
    await notion.blocks_append("<block_id>", [
        {"object": "block", "type": "paragraph",
         "paragraph": {"rich_text": [{"text": {"content": "hello"}}]}},
    ])

    # Update a page's properties
    await notion.page_update("<page_id>", {"In stock": {"checkbox": True}})

All are async (kernel-loop style: no blocking network calls on the shared event
loop) and wrap the Notion REST API (https://developers.notion.com/reference/intro).

``NOTION_API_KEY`` is read from ``os.environ`` at call time so a session that
sets the key after import still works. A missing key raises ``RuntimeError``
with a clear message. Notion error envelopes
(``{"object": "error", "status": ..., "code": ..., "message": ...}``) surface as
:class:`NotionError` rather than silently returning ``None`` data.

The ``_client`` hook (see below) lets tests inject an ``httpx.MockTransport``
so every code path is exercisable with no network.
"""

from __future__ import annotations

import os
from typing import TYPE_CHECKING, Any

import polars as pl
from pydantic import BaseModel, ConfigDict

if TYPE_CHECKING:
    import httpx

__all__ = [
    "NotionError",
    "Page",
    "blocks",
    "blocks_append",
    "db_query",
    "page",
    "page_create",
    "page_update",
    "search",
]

__version__ = "0.1.0"

# The Notion REST API base. Every endpoint below is appended to this.
_BASE_URL = "https://api.notion.com/v1"

# The required ``Notion-Version`` header. Notion versions are dated; 2022-06-28
# is the long-standing stable default that keeps the flat ``/v1/databases/{id}/
# query`` surface and the classic page-property shapes. Newer dated versions
# (2025-09-03, 2026-03-11) introduce breaking renames (data_sources, in_trash)
# that this thin wrapper deliberately does not track. The header is required on
# every request -- Notion 400s without it.
_NOTION_VERSION = "2022-06-28"

# How many rows a paginated list endpoint pulls per request (Notion's max).
_PAGE_SIZE = 100


# ---------------------------------------------------------------------------
# Response models
# ---------------------------------------------------------------------------
#
# Notion responses are parsed into these pydantic models at the boundary (in the
# public functions below) rather than passed around as untyped dicts. ``extra=
# "ignore"`` keeps the models forward-compatible if Notion adds fields, and lets
# the same model absorb the differing selection a create/update returns.


class _NotionModel(BaseModel):
    model_config = ConfigDict(extra="ignore")


class Page(_NotionModel):
    """A Notion page object.

    ``properties`` and ``parent`` are kept as raw dicts because their shape is
    schema-dependent (each database defines its own property names and types);
    callers index into them directly.
    """

    id: str
    object: str | None = None
    url: str | None = None
    created_time: str | None = None
    last_edited_time: str | None = None
    archived: bool | None = None
    parent: dict[str, Any] | None = None
    properties: dict[str, Any] = {}


# Fixed schemas so empty results stay typed (polars cannot infer a schema from
# zero rows). ``search``/``db_query`` return whole objects too unwieldy for a
# flat frame, so each row is the stable identifying columns plus the raw object.
_SEARCH_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "object": pl.Utf8,
    "title": pl.Utf8,
    "url": pl.Utf8,
    "created_time": pl.Utf8,
    "last_edited_time": pl.Utf8,
}

_BLOCKS_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "type": pl.Utf8,
    "has_children": pl.Boolean,
    "created_time": pl.Utf8,
    "last_edited_time": pl.Utf8,
    "text": pl.Utf8,
}

_DB_QUERY_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "id": pl.Utf8,
    "object": pl.Utf8,
    "title": pl.Utf8,
    "url": pl.Utf8,
    "created_time": pl.Utf8,
    "last_edited_time": pl.Utf8,
}


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


class NotionError(RuntimeError):
    """Raised when the Notion REST response is an error envelope.

    Notion returns ``{"object": "error", "status": ..., "code": ...,
    "message": ...}`` on failure. The ``status``, ``code``, and ``message`` are
    exposed as attributes so callers can branch on the code (e.g.
    ``"object_not_found"``, ``"unauthorized"``, ``"validation_error"``) without
    parsing the exception message.
    """

    def __init__(self, envelope: dict[str, Any]) -> None:
        self.status = envelope.get("status")
        self.code = envelope.get("code")
        self.message = envelope.get("message", "")
        self.envelope = envelope
        super().__init__(f"Notion API error ({self.code}): {self.message}")


def _api_key() -> str:
    """Return the Notion API key from the environment or raise clearly."""
    key = os.environ.get("NOTION_API_KEY", "")
    if not key:
        raise RuntimeError(
            "NOTION_API_KEY is not set in the environment. "
            "Provision the key (https://www.notion.so/my-integrations) and retry."
        )
    return key


# _client is module-level so tests can replace it with a factory that injects
# httpx.MockTransport without patching internals:
#
#   import notion, httpx
#   notion._client = lambda **kw: httpx.AsyncClient(
#       transport=httpx.MockTransport(handler), **kw
#   )
#
# Production code calls _client() each time so that a key set after import
# (common in notebooks) is always picked up.
def _client(**kwargs: Any) -> httpx.AsyncClient:  # noqa: ANN401 -- forwarded verbatim to httpx.AsyncClient
    """Return a fresh ``httpx.AsyncClient`` wired for the Notion REST API.

    Keyword arguments are forwarded to the constructor, letting callers (and
    tests) override ``base_url``, ``transport``, etc.
    """
    import httpx

    key = _api_key()
    headers = {
        "Authorization": f"Bearer {key}",
        "Notion-Version": _NOTION_VERSION,
        "Content-Type": "application/json",
    }
    return httpx.AsyncClient(base_url=_BASE_URL, headers=headers, **kwargs)


async def _request(
    method: str,
    path: str,
    *,
    params: dict[str, Any] | None = None,
    json: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Execute one Notion REST call and return the decoded JSON body.

    ``params`` are URL query parameters (httpx encodes them, so an opaque
    pagination cursor with reserved characters stays intact -- never build the
    query string by hand). ``json`` is the request body.

    Raises :class:`NotionError` when the body is a Notion error envelope
    (``{"object": "error", ...}``), regardless of the HTTP status code, so the
    actionable Notion ``code``/``message`` always wins over a bare
    ``HTTPStatusError``. ``httpx.HTTPStatusError`` still surfaces for a non-2xx
    response whose body is *not* a parseable error envelope (e.g. a proxy 502).
    """
    import httpx

    async with _client() as client:
        resp = await client.request(method, path, params=params, json=json)
        try:
            body: dict[str, Any] = resp.json()
        except ValueError:
            resp.raise_for_status()
            raise
    if isinstance(body, dict) and body.get("object") == "error":
        raise NotionError(body)
    if resp.status_code >= httpx.codes.BAD_REQUEST:
        resp.raise_for_status()
    return body


def _plain_text(rich_text: list[dict[str, Any]] | None) -> str:
    """Flatten a Notion ``rich_text`` array to its concatenated plain text."""
    if not rich_text:
        return ""
    return "".join(rt.get("plain_text", "") for rt in rich_text)


def _object_title(obj: dict[str, Any]) -> str:
    """Best-effort plain-text title for a page or database object.

    A database carries a top-level ``title`` rich-text array; a page carries its
    title inside the ``title``-typed entry of ``properties`` (whose key is the
    schema-defined name, so it is found by type, not by name).
    """
    if obj.get("object") == "database":
        return _plain_text(obj.get("title"))
    for prop in (obj.get("properties") or {}).values():
        if isinstance(prop, dict) and prop.get("type") == "title":
            return _plain_text(prop.get("title"))
    return ""


def _block_text(block: dict[str, Any]) -> str:
    """Best-effort plain text for a block (its ``rich_text``, if it has one)."""
    body = block.get(block.get("type", ""), {})
    if isinstance(body, dict):
        return _plain_text(body.get("rich_text"))
    return ""


def _object_row(obj: dict[str, Any]) -> dict[str, Any]:
    """The fixed-schema row for a page/database object (search + db_query)."""
    return {
        "id": obj.get("id", ""),
        "object": obj.get("object", ""),
        "title": _object_title(obj),
        "url": obj.get("url", ""),
        "created_time": obj.get("created_time", ""),
        "last_edited_time": obj.get("last_edited_time", ""),
    }


def _block_row(block: dict[str, Any]) -> dict[str, Any]:
    """The fixed-schema row for a block (blocks + blocks_append)."""
    return {
        "id": block.get("id", ""),
        "type": block.get("type", ""),
        "has_children": bool(block.get("has_children")),
        "created_time": block.get("created_time", ""),
        "last_edited_time": block.get("last_edited_time", ""),
        "text": _block_text(block),
    }


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


async def search(query: str = "", filter: str | None = None) -> pl.DataFrame:
    """Search pages and databases the integration can see, as a polars DataFrame.

    ``query`` is matched against page and database titles; an empty string
    returns everything shared with the integration. Pass ``filter="page"`` or
    ``filter="database"`` to restrict the object type (Notion's ``filter`` takes
    ``{"property": "object", "value": ...}``).

    Columns: ``id``, ``object`` (``"page"`` / ``"database"``), ``title``,
    ``url``, ``created_time``, ``last_edited_time``. Results are paginated
    automatically. Raises :class:`NotionError` on API errors and ``RuntimeError``
    when ``NOTION_API_KEY`` is not set.
    """
    payload: dict[str, Any] = {"page_size": _PAGE_SIZE}
    if query:
        payload["query"] = query
    if filter is not None:
        payload["filter"] = {"property": "object", "value": filter}

    rows: list[dict[str, Any]] = []
    cursor: str | None = None
    while True:
        if cursor:
            payload["start_cursor"] = cursor
        data = await _request("POST", "/search", json=payload)
        rows.extend(_object_row(obj) for obj in data.get("results", []))
        cursor = data.get("next_cursor")
        if not data.get("has_more") or not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_SEARCH_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_SEARCH_SCHEMA).select(list(_SEARCH_SCHEMA))


async def page(page_id: str) -> Page:
    """Fetch a Notion page by id.

    Returns a typed :class:`Page` with attribute access::

        p = await notion.page("<page_id>")
        p.url                    # str
        p.properties["Name"]     # the raw property value

    Raises :class:`NotionError` if no page has that id (Notion returns an
    ``object_not_found`` envelope) or on other API errors, and ``RuntimeError``
    when ``NOTION_API_KEY`` is not set.
    """
    data = await _request("GET", f"/pages/{page_id}")
    return Page.model_validate(data)


async def blocks(block_id: str) -> pl.DataFrame:
    """A block's (or page's) child blocks, as a polars DataFrame.

    ``block_id`` is a block id or a page id (a page is itself a block). Columns:
    ``id``, ``type`` (e.g. ``"paragraph"``, ``"heading_1"``, ``"to_do"``),
    ``has_children``, ``created_time``, ``last_edited_time``, ``text`` (the
    block's flattened ``rich_text`` plain text, empty for non-text blocks).
    Results are paginated automatically.

    Raises :class:`NotionError` on API errors and ``RuntimeError`` when
    ``NOTION_API_KEY`` is not set.
    """
    rows: list[dict[str, Any]] = []
    cursor: str | None = None
    while True:
        # The cursor is an opaque token, so it goes through httpx's `params`
        # (URL-encoded) rather than into a hand-built query string.
        params: dict[str, Any] = {"page_size": _PAGE_SIZE}
        if cursor:
            params["start_cursor"] = cursor
        data = await _request("GET", f"/blocks/{block_id}/children", params=params)
        rows.extend(_block_row(block) for block in data.get("results", []))
        cursor = data.get("next_cursor")
        if not data.get("has_more") or not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_BLOCKS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_BLOCKS_SCHEMA).select(list(_BLOCKS_SCHEMA))


async def db_query(
    database_id: str,
    filter: dict[str, Any] | None = None,
    sorts: list[dict[str, Any]] | None = None,
) -> pl.DataFrame:
    """Query a Notion database and return matching pages as a polars DataFrame.

    ``filter`` and ``sorts`` are passed through verbatim to Notion's database
    query (see https://developers.notion.com/reference/post-database-query), e.g.
    ``filter={"property": "Status", "select": {"equals": "Done"}}`` and
    ``sorts=[{"property": "Name", "direction": "ascending"}]``.

    Columns: ``id``, ``object``, ``title``, ``url``, ``created_time``,
    ``last_edited_time`` (one row per page in the database). Results are paginated
    automatically. Raises :class:`NotionError` on API errors and ``RuntimeError``
    when ``NOTION_API_KEY`` is not set.
    """
    payload: dict[str, Any] = {"page_size": _PAGE_SIZE}
    if filter is not None:
        payload["filter"] = filter
    if sorts is not None:
        payload["sorts"] = sorts

    rows: list[dict[str, Any]] = []
    cursor: str | None = None
    while True:
        if cursor:
            payload["start_cursor"] = cursor
        data = await _request("POST", f"/databases/{database_id}/query", json=payload)
        rows.extend(_object_row(obj) for obj in data.get("results", []))
        cursor = data.get("next_cursor")
        if not data.get("has_more") or not cursor:
            break

    if not rows:
        return pl.DataFrame(schema=_DB_QUERY_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_DB_QUERY_SCHEMA).select(
        list(_DB_QUERY_SCHEMA)
    )


async def page_create(
    parent: dict[str, Any],
    properties: dict[str, Any],
    children: list[dict[str, Any]] | None = None,
) -> Page:
    """Create a new Notion page and return the created :class:`Page`.

    ``parent`` names where the page goes, e.g.
    ``{"database_id": "<id>"}`` or ``{"page_id": "<id>"}``.
    ``properties`` is the Notion property map; a database page must include the
    database's title property, e.g.
    ``{"Name": {"title": [{"text": {"content": "New row"}}]}}``.
    ``children`` is an optional list of block objects for the page body.

    Raises :class:`NotionError` on API errors (e.g. ``validation_error`` for a
    property that does not match the database schema) and ``RuntimeError`` when
    ``NOTION_API_KEY`` is not set.
    """
    payload: dict[str, Any] = {"parent": parent, "properties": properties}
    if children is not None:
        payload["children"] = children
    data = await _request("POST", "/pages", json=payload)
    return Page.model_validate(data)


async def blocks_append(
    block_id: str,
    children: list[dict[str, Any]],
) -> pl.DataFrame:
    """Append ``children`` blocks to a block (or page) and return the new blocks.

    ``block_id`` is the parent block or page id. ``children`` is a list of Notion
    block objects, e.g.::

        await notion.blocks_append("<page_id>", [
            {"object": "block", "type": "paragraph",
             "paragraph": {"rich_text": [{"text": {"content": "hi"}}]}},
        ])

    Returns a polars DataFrame of the appended blocks (same columns as
    :func:`blocks`). Raises :class:`NotionError` on API errors and
    ``RuntimeError`` when ``NOTION_API_KEY`` is not set.
    """
    data = await _request(
        "PATCH", f"/blocks/{block_id}/children", json={"children": children}
    )
    rows = [_block_row(block) for block in data.get("results", [])]
    if not rows:
        return pl.DataFrame(schema=_BLOCKS_SCHEMA)
    return pl.DataFrame(rows, schema_overrides=_BLOCKS_SCHEMA).select(list(_BLOCKS_SCHEMA))


async def page_update(page_id: str, properties: dict[str, Any]) -> Page:
    """Update a Notion page's properties and return the updated :class:`Page`.

    ``properties`` is the partial Notion property map to change, e.g.
    ``{"In stock": {"checkbox": True}}`` or
    ``{"Status": {"select": {"name": "Done"}}}``.

    Raises :class:`NotionError` on API errors (e.g. ``validation_error``) and
    ``RuntimeError`` when ``NOTION_API_KEY`` is not set.
    """
    data = await _request("PATCH", f"/pages/{page_id}", json={"properties": properties})
    return Page.model_validate(data)
