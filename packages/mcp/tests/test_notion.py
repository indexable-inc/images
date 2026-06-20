"""Network-free tests for the `notion` helper.

These never reach Notion: every code path is exercised with an
``httpx.MockTransport`` injected via the module's ``_client`` hook, so there is
no network and no real token. They cover: the missing-key RuntimeError, a
successful search, a paginated db_query, a page_create, and the error-envelope
to NotionError mapping.
"""

from __future__ import annotations

import asyncio
import inspect
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pytest

# Prefer the bundled module (nix check); fall back to the source tree (dev run).
NOTION_SRC = Path(__file__).resolve().parents[1] / "src" / "notion"
if NOTION_SRC.is_dir() and str(NOTION_SRC) not in sys.path:
    sys.path.insert(0, str(NOTION_SRC))

import notion

# Public callables = everything exported except the error class and the model.
_NON_CALLABLE = {"NotionError", "Page"}
_PUBLIC_FUNCS = [getattr(notion, name) for name in notion.__all__ if name not in _NON_CALLABLE]


def test_all_names_exist() -> None:
    for name in notion.__all__:
        assert hasattr(notion, name), f"{name} in __all__ but missing from module"


def test_error_type() -> None:
    assert issubclass(notion.NotionError, RuntimeError)


def test_public_funcs_are_async() -> None:
    for func in _PUBLIC_FUNCS:
        assert asyncio.iscoroutinefunction(func), f"{func.__name__} is not async"


def test_type_hints_explicit() -> None:
    # Mirrors the ruff ANN gate: every public function fully annotates its params
    # and return type.
    for func in _PUBLIC_FUNCS:
        sig = inspect.signature(func)
        assert sig.return_annotation is not inspect.Signature.empty, (
            f"{func.__name__} missing return annotation"
        )
        for pname, param in sig.parameters.items():
            assert param.annotation is not inspect.Parameter.empty, (
                f"{func.__name__}({pname}) missing annotation"
            )


def _install_handler(
    monkeypatch: pytest.MonkeyPatch,
    handler: Callable[[httpx.Request], httpx.Response],
    *,
    key: str = "secret_test",
) -> list[httpx.Request]:
    """Wire ``notion._client`` to a MockTransport running ``handler``.

    Returns the list the handler appends each received request to, so a test can
    assert on the method/path/body that the module actually sent. A token is set
    so ``_client`` (which calls ``_api_key``) does not raise.
    """
    monkeypatch.setenv("NOTION_API_KEY", key)
    seen: list[httpx.Request] = []

    def wrapped(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return handler(request)

    def make_client() -> httpx.AsyncClient:
        # Mirror production _client wiring (base_url + the required headers) so
        # the header assertions exercise the real shape, but swap the network
        # transport for the MockTransport.
        return httpx.AsyncClient(
            base_url=notion._BASE_URL,
            headers={
                "Authorization": f"Bearer {key}",
                "Notion-Version": notion._NOTION_VERSION,
                "Content-Type": "application/json",
            },
            transport=httpx.MockTransport(wrapped),
        )

    monkeypatch.setattr(notion, "_client", make_client)
    return seen


def test_missing_key_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("NOTION_API_KEY", raising=False)
    with pytest.raises(RuntimeError, match="NOTION_API_KEY"):
        asyncio.run(notion.search("anything"))


def test_search_returns_typed_frame(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "object": "list",
                "results": [
                    {
                        "object": "page",
                        "id": "page-1",
                        "url": "https://notion.so/page-1",
                        "created_time": "2026-01-01T00:00:00.000Z",
                        "last_edited_time": "2026-01-02T00:00:00.000Z",
                        "properties": {
                            "Name": {
                                "type": "title",
                                "title": [{"plain_text": "Roadmap"}],
                            }
                        },
                    },
                    {
                        "object": "database",
                        "id": "db-1",
                        "url": "https://notion.so/db-1",
                        "created_time": "2026-01-03T00:00:00.000Z",
                        "last_edited_time": "2026-01-04T00:00:00.000Z",
                        "title": [{"plain_text": "Tasks DB"}],
                    },
                ],
                "has_more": False,
                "next_cursor": None,
            },
        )

    seen = _install_handler(monkeypatch, handler)
    df = asyncio.run(notion.search("road", filter="page"))

    # The request shape the module sent.
    assert seen[-1].method == "POST"
    assert seen[-1].url.path == "/v1/search"
    assert seen[-1].headers["Notion-Version"] == notion._NOTION_VERSION
    assert seen[-1].headers["Authorization"] == "Bearer secret_test"

    # The frame: titles flattened from both a page (via properties) and a
    # database (via top-level title), columns in the fixed schema order.
    assert df.columns == list(notion._SEARCH_SCHEMA)
    assert df["title"].to_list() == ["Roadmap", "Tasks DB"]
    assert df["object"].to_list() == ["page", "database"]


def test_db_query_paginates(monkeypatch: pytest.MonkeyPatch) -> None:
    pages = [
        {
            "object": "list",
            "results": [
                {
                    "object": "page",
                    "id": "row-1",
                    "url": "u1",
                    "created_time": "t1",
                    "last_edited_time": "t1",
                    "properties": {"Name": {"type": "title", "title": [{"plain_text": "A"}]}},
                }
            ],
            "has_more": True,
            "next_cursor": "cursor-2",
        },
        {
            "object": "list",
            "results": [
                {
                    "object": "page",
                    "id": "row-2",
                    "url": "u2",
                    "created_time": "t2",
                    "last_edited_time": "t2",
                    "properties": {"Name": {"type": "title", "title": [{"plain_text": "B"}]}},
                }
            ],
            "has_more": False,
            "next_cursor": None,
        },
    ]
    bodies: list[dict[str, Any]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        import json as _json

        bodies.append(_json.loads(request.content))
        return httpx.Response(200, json=pages[len(bodies) - 1])

    seen = _install_handler(monkeypatch, handler)
    df = asyncio.run(notion.db_query("db-1", sorts=[{"property": "Name", "direction": "ascending"}]))

    # Two requests: the second carries the start_cursor from the first response.
    assert len(seen) == 2
    assert seen[0].url.path == "/v1/databases/db-1/query"
    assert "start_cursor" not in bodies[0]
    assert bodies[0]["sorts"] == [{"property": "Name", "direction": "ascending"}]
    assert bodies[1]["start_cursor"] == "cursor-2"

    # Both pages' rows are concatenated, in order.
    assert df["id"].to_list() == ["row-1", "row-2"]
    assert df["title"].to_list() == ["A", "B"]


def test_page_create_returns_model(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "object": "page",
                "id": "new-page",
                "url": "https://notion.so/new-page",
                "parent": {"database_id": "db-1"},
                "properties": {"Name": {"type": "title", "title": [{"plain_text": "Hi"}]}},
            },
        )

    seen = _install_handler(monkeypatch, handler)
    created = asyncio.run(
        notion.page_create(
            {"database_id": "db-1"},
            {"Name": {"title": [{"text": {"content": "Hi"}}]}},
        )
    )

    assert seen[-1].method == "POST"
    assert seen[-1].url.path == "/v1/pages"
    assert isinstance(created, notion.Page)
    assert created.id == "new-page"
    assert created.url == "https://notion.so/new-page"


def test_blocks_paginates_with_encoded_cursor(monkeypatch: pytest.MonkeyPatch) -> None:
    # A cursor with reserved characters must survive intact: it goes through
    # httpx params (URL-encoded), never a hand-built query string. If the module
    # interpolated it raw, the `&`/`=` would split into bogus query params and
    # this round-trip would fetch the wrong page (or loop).
    tricky_cursor = "a&b=c d+e/f"
    pages = [
        {
            "object": "list",
            "results": [{"object": "block", "id": "b-1", "type": "paragraph"}],
            "has_more": True,
            "next_cursor": tricky_cursor,
        },
        {
            "object": "list",
            "results": [{"object": "block", "id": "b-2", "type": "paragraph"}],
            "has_more": False,
            "next_cursor": None,
        },
    ]
    calls: list[int] = []

    def handler(request: httpx.Request) -> httpx.Response:
        calls.append(1)
        return httpx.Response(200, json=pages[len(calls) - 1])

    seen = _install_handler(monkeypatch, handler)
    df = asyncio.run(notion.blocks("page-1"))

    assert len(seen) == 2
    assert seen[0].url.path == "/v1/blocks/page-1/children"
    # The second request carries the cursor decoded back to its exact value --
    # proving it was encoded on the way out, not split into junk params.
    assert "start_cursor" not in seen[0].url.params
    assert seen[1].url.params["start_cursor"] == tricky_cursor
    assert df["id"].to_list() == ["b-1", "b-2"]


def test_blocks_append_returns_new_blocks(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "object": "list",
                "results": [
                    {
                        "object": "block",
                        "id": "new-block",
                        "type": "paragraph",
                        "paragraph": {"rich_text": [{"plain_text": "hello"}]},
                    }
                ],
            },
        )

    seen = _install_handler(monkeypatch, handler)
    df = asyncio.run(
        notion.blocks_append(
            "page-1",
            [
                {
                    "object": "block",
                    "type": "paragraph",
                    "paragraph": {"rich_text": [{"text": {"content": "hello"}}]},
                }
            ],
        )
    )

    assert seen[-1].method == "PATCH"
    assert seen[-1].url.path == "/v1/blocks/page-1/children"
    assert df["id"].to_list() == ["new-block"]
    assert df["text"].to_list() == ["hello"]


def test_page_update_returns_model(monkeypatch: pytest.MonkeyPatch) -> None:
    bodies: list[dict[str, Any]] = []

    def handler(request: httpx.Request) -> httpx.Response:
        import json as _json

        bodies.append(_json.loads(request.content))
        return httpx.Response(
            200,
            json={"object": "page", "id": "p-1", "url": "https://notion.so/p-1", "properties": {}},
        )

    seen = _install_handler(monkeypatch, handler)
    updated = asyncio.run(notion.page_update("p-1", {"In stock": {"checkbox": True}}))

    assert seen[-1].method == "PATCH"
    assert seen[-1].url.path == "/v1/pages/p-1"
    assert bodies[-1] == {"properties": {"In stock": {"checkbox": True}}}
    assert isinstance(updated, notion.Page)
    assert updated.id == "p-1"


def test_error_envelope_raises_notion_error(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(request: httpx.Request) -> httpx.Response:
        # Notion returns the error envelope with a non-2xx status; the module
        # must surface the envelope's code/message, not a bare HTTPStatusError.
        return httpx.Response(
            404,
            json={
                "object": "error",
                "status": 404,
                "code": "object_not_found",
                "message": "Could not find page with ID: missing.",
            },
        )

    _install_handler(monkeypatch, handler)
    with pytest.raises(notion.NotionError) as excinfo:
        asyncio.run(notion.page("missing"))

    err = excinfo.value
    assert err.code == "object_not_found"
    assert err.status == 404
    assert "Could not find page" in err.message
    assert "object_not_found" in str(err)
