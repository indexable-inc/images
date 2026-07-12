"""Network-free tests for the `sourcegraph` helper.

These never reach sourcegraph.com: every code path is exercised with an
``httpx.MockTransport`` injected via the module's ``_client`` hook, so there is
no network and no token. They cover: anonymous-by-default auth (and the
``SRC_ACCESS_TOKEN`` header when set), the ``count:`` append/preserve rule, the
FileMatch/Repository/CommitSearchResult row shapes, the relative-to-absolute
``url`` normalization, the fixed empty-frame schema, the error-envelope to
SourcegraphError mapping, and the 5xx retry.
"""

from __future__ import annotations

import asyncio
import inspect
import json
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import polars as pl
import pytest

# Prefer the bundled module (nix check); fall back to the source tree (dev run).
SOURCEGRAPH_SRC = Path(__file__).resolve().parents[1] / "src" / "sourcegraph"
if SOURCEGRAPH_SRC.is_dir() and str(SOURCEGRAPH_SRC) not in sys.path:
    sys.path.insert(0, str(SOURCEGRAPH_SRC))

import sourcegraph

_NON_CALLABLE = {"SourcegraphError"}
_PUBLIC_FUNCS = [
    getattr(sourcegraph, name) for name in sourcegraph.__all__ if name not in _NON_CALLABLE
]


def test_all_names_exist() -> None:
    for name in sourcegraph.__all__:
        assert hasattr(sourcegraph, name), f"{name} in __all__ but missing from module"


def test_error_type() -> None:
    assert issubclass(sourcegraph.SourcegraphError, RuntimeError)


def test_public_funcs_are_async() -> None:
    for func in _PUBLIC_FUNCS:
        assert asyncio.iscoroutinefunction(func), f"{func.__name__} is not async"


@pytest.mark.parametrize("func", _PUBLIC_FUNCS, ids=lambda f: str(f.__name__))
def test_type_hints_explicit(func: Callable[..., object]) -> None:
    # Mirrors the ruff ANN gate: every public function fully annotates its params
    # and return type.
    sig = inspect.signature(func)
    unannotated = [
        pname
        for pname, param in sig.parameters.items()
        if param.annotation is inspect.Parameter.empty
    ]
    assert not unannotated, f"{func.__name__} params missing annotations: {unannotated}"
    assert sig.return_annotation is not inspect.Signature.empty, (
        f"{func.__name__} missing return annotation"
    )


def _envelope(results: list[dict[str, Any]], *, match_count: int | None = None) -> dict[str, Any]:
    """A GraphQL success envelope shaped like the live search response."""
    return {
        "data": {
            "search": {
                "results": {
                    "matchCount": len(results) if match_count is None else match_count,
                    "limitHit": False,
                    "results": results,
                }
            }
        }
    }


_FILE_MATCH: dict[str, Any] = {
    "__typename": "FileMatch",
    "repository": {"name": "github.com/rust-lang/rust", "stars": 95000},
    "file": {
        "path": "library/core/src/mem/mod.rs",
        "url": "/github.com/rust-lang/rust/-/blob/library/core/src/mem/mod.rs",
        "commit": {"oid": "abc123def4567890abc123def4567890abc123de"},
    },
    "lineMatches": [
        {"preview": "pub unsafe fn transmute<T, U>(src: T) -> U {", "lineNumber": 41},
        {"preview": "    unsafe { transmute_copy(&src) }", "lineNumber": 99},
    ],
}

_REPO_MATCH: dict[str, Any] = {
    "__typename": "Repository",
    "name": "github.com/pola-rs/polars",
    "stars": 30000,
    "description": "Dataframes powered by a multithreaded query engine",
    "url": "/github.com/pola-rs/polars",
}

_COMMIT_MATCH: dict[str, Any] = {
    "__typename": "CommitSearchResult",
    "url": "/github.com/rust-lang/rust/-/commit/feedface",
    "commit": {
        "oid": "feedfacefeedfacefeedfacefeedfacefeedface",
        "subject": "Stabilize transmute in const fn",
        "repository": {"name": "github.com/rust-lang/rust", "stars": 95000},
    },
}


def _install_handler(
    monkeypatch: pytest.MonkeyPatch,
    handler: Callable[[httpx.Request], httpx.Response],
) -> list[httpx.Request]:
    """Wire ``sourcegraph._client`` to a MockTransport running ``handler``.

    Returns the list the handler appends each received request to, so a test
    can assert on the URL/body/headers the module actually sent. The factory
    mirrors production ``_client`` wiring (env-driven base_url + anonymous vs
    ``SRC_ACCESS_TOKEN`` headers) so the auth assertions exercise the real
    shape, but swaps the network transport for the MockTransport.
    """
    seen: list[httpx.Request] = []

    def wrapped(request: httpx.Request) -> httpx.Response:
        seen.append(request)
        return handler(request)

    def make_client(**kwargs: Any) -> httpx.AsyncClient:  # noqa: ANN401 -- mirrors the production hook
        import os

        headers = {"Content-Type": "application/json"}
        token = os.environ.get("SRC_ACCESS_TOKEN", "")
        if token:
            headers["Authorization"] = f"token {token}"
        return httpx.AsyncClient(
            base_url=sourcegraph._endpoint(),
            headers=headers,
            transport=httpx.MockTransport(wrapped),
            **kwargs,
        )

    monkeypatch.setattr(sourcegraph, "_client", make_client)
    return seen


def test_search_rows_and_schema(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    monkeypatch.delenv("SRC_ENDPOINT", raising=False)
    seen = _install_handler(
        monkeypatch,
        lambda _req: httpx.Response(
            200, json=_envelope([_FILE_MATCH, _REPO_MATCH, _COMMIT_MATCH])
        ),
    )

    df = asyncio.run(sourcegraph.search("transmute lang:rust"))

    assert list(df.columns) == list(sourcegraph._SEARCH_SCHEMA)
    # Two line matches + one repo + one commit = 4 rows.
    assert df.height == 4
    content = df.filter(pl.col("kind") == "content")
    assert content.height == 2
    row = content.row(0, named=True)
    assert row["repo"] == "github.com/rust-lang/rust"
    assert row["stars"] == 95000
    assert row["path"] == "library/core/src/mem/mod.rs"
    assert row["line"] == 42  # 0-based lineNumber 41 -> 1-based
    assert "transmute" in row["content"]
    assert row["commit"].startswith("abc123")
    # The API's instance-relative url is absolutized against the endpoint.
    assert row["url"] == (
        "https://sourcegraph.com/github.com/rust-lang/rust/-/blob/library/core/src/mem/mod.rs"
    )
    repo_row = df.filter(pl.col("kind") == "repo").row(0, named=True)
    assert repo_row["repo"] == "github.com/pola-rs/polars"
    assert repo_row["path"] is None
    assert "Dataframes" in repo_row["content"]
    assert repo_row["url"] == "https://sourcegraph.com/github.com/pola-rs/polars"
    commit_row = df.filter(pl.col("kind") == "commit").row(0, named=True)
    assert commit_row["commit"].startswith("feedface")
    assert commit_row["content"] == "Stabilize transmute in const fn"
    assert commit_row["url"] == "https://sourcegraph.com/github.com/rust-lang/rust/-/commit/feedface"

    # The request went to the GraphQL endpoint, anonymously.
    (request,) = seen
    assert request.url.path == "/.api/graphql"
    assert "authorization" not in request.headers


def test_count_appended_and_preserved(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    seen = _install_handler(monkeypatch, lambda _req: httpx.Response(200, json=_envelope([])))

    asyncio.run(sourcegraph.search("foo", first=7))
    asyncio.run(sourcegraph.search("foo count:all"))

    first_query = json.loads(seen[0].content)["variables"]["query"]
    second_query = json.loads(seen[1].content)["variables"]["query"]
    assert first_query == "foo count:7"
    assert second_query == "foo count:all"  # caller's count: wins, nothing appended


def test_token_sent_when_set(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SRC_ACCESS_TOKEN", "sgp_test_token")
    seen = _install_handler(monkeypatch, lambda _req: httpx.Response(200, json=_envelope([])))

    asyncio.run(sourcegraph.search("foo"))

    (request,) = seen
    assert request.headers["Authorization"] == "token sgp_test_token"


def test_endpoint_override(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    monkeypatch.setenv("SRC_ENDPOINT", "https://sourcegraph.example.com/")
    seen = _install_handler(monkeypatch, lambda _req: httpx.Response(200, json=_envelope([])))

    asyncio.run(sourcegraph.search("foo"))

    (request,) = seen
    assert request.url.host == "sourcegraph.example.com"
    assert request.url.path == "/.api/graphql"  # trailing slash stripped, no `//`


def test_urls_absolutized_against_custom_endpoint(monkeypatch: pytest.MonkeyPatch) -> None:
    # Relative API urls resolve against the configured instance, not the
    # sourcegraph.com default; already-absolute urls pass through untouched.
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    monkeypatch.setenv("SRC_ENDPOINT", "https://sourcegraph.example.com/")
    absolute_repo = {**_REPO_MATCH, "url": "https://mirror.example.net/github.com/pola-rs/polars"}
    _install_handler(
        monkeypatch,
        lambda _req: httpx.Response(200, json=_envelope([_COMMIT_MATCH, absolute_repo])),
    )

    df = asyncio.run(sourcegraph.search("foo"))

    urls = dict(zip(df["kind"], df["url"], strict=True))
    assert urls["commit"] == (
        "https://sourcegraph.example.com/github.com/rust-lang/rust/-/commit/feedface"
    )
    assert urls["repo"] == "https://mirror.example.net/github.com/pola-rs/polars"


def test_empty_results_keep_schema(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    _install_handler(monkeypatch, lambda _req: httpx.Response(200, json=_envelope([])))

    df = asyncio.run(sourcegraph.search("no such thing anywhere"))

    assert df.height == 0
    assert df.schema["stars"] == pl.Int64
    assert df.schema["line"] == pl.Int64
    assert df.schema["content"] == pl.Utf8


def test_path_only_file_match(monkeypatch: pytest.MonkeyPatch) -> None:
    # A `type:path` hit has no lineMatches but must still produce a row.
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    path_match = {**_FILE_MATCH, "lineMatches": []}
    _install_handler(monkeypatch, lambda _req: httpx.Response(200, json=_envelope([path_match])))

    df = asyncio.run(sourcegraph.search("type:path mod.rs"))

    assert df.height == 1
    row = df.row(0, named=True)
    assert row["path"] == "library/core/src/mem/mod.rs"
    assert row["line"] is None
    assert row["content"] is None


def test_graphql_errors_raise(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    _install_handler(
        monkeypatch,
        lambda _req: httpx.Response(
            200, json={"errors": [{"message": "invalid query syntax"}], "data": None}
        ),
    )

    with pytest.raises(sourcegraph.SourcegraphError, match="invalid query syntax"):
        asyncio.run(sourcegraph.search("("))


def test_http_5xx_retries_then_succeeds(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    # Zero the backoffs so the retry path runs instantly.
    monkeypatch.setattr(sourcegraph, "_RETRY_BACKOFFS_S", (0.0, 0.0))
    calls = {"n": 0}

    def flaky(_req: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        if calls["n"] == 1:
            return httpx.Response(502, text="bad gateway")
        return httpx.Response(200, json=_envelope([_REPO_MATCH]))

    _install_handler(monkeypatch, flaky)

    df = asyncio.run(sourcegraph.search("type:repo polars"))

    assert calls["n"] == 2
    assert df.height == 1


def test_http_4xx_raises_without_retry(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("SRC_ACCESS_TOKEN", raising=False)
    calls = {"n": 0}

    def denied(_req: httpx.Request) -> httpx.Response:
        calls["n"] += 1
        return httpx.Response(401, text="unauthorized")

    _install_handler(monkeypatch, denied)

    with pytest.raises(httpx.HTTPStatusError):
        asyncio.run(sourcegraph.search("foo"))
    assert calls["n"] == 1
