"""Global public code search (GitHub and beyond) over the Sourcegraph GraphQL API.

Bundled like ``linear``/``notion`` so every session can ``import sourcegraph``
and search the world's public code without hand-rolling an httpx client each
time.  Anonymous by default -- sourcegraph.com serves public search without a
token -- with ``SRC_ACCESS_TOKEN`` honored when present (higher rate limits,
private code on a private instance)::

    import sourcegraph

    # One row per matched line (plus repo/commit hits), as a polars DataFrame
    df = await sourcegraph.search("lang:rust unsafe fn transmute")
    df.filter(pl.col("stars") > 1000).select("repo", "path", "line", "content")

    # Sourcegraph's query language passes straight through
    await sourcegraph.search(r"repo:^github\\.com/rust-lang/rust$ patterntype:regexp fn\\s+main")
    await sourcegraph.search("type:repo topic:database", first=100)

The returned frame has a fixed schema, one row per match::

    kind    -- "content" (a matched line), "repo", or "commit"
    repo    -- repository name, e.g. "github.com/rust-lang/rust"
    stars   -- repository star count
    path    -- file path for content matches (null for repo/commit rows)
    line    -- 1-based line number for content matches
    content -- the matched line / repo description / commit subject
    commit  -- commit OID the match was found at
    url     -- Sourcegraph URL of the match

``search`` is async (kernel-loop style: no blocking network calls on the
shared event loop) and wraps the Sourcegraph GraphQL search API
(https://sourcegraph.com/docs/api/graphql).  Result caps ride the query itself:
``first=`` appends ``count:N`` unless the query already carries one.

``SRC_ACCESS_TOKEN`` and ``SRC_ENDPOINT`` (default ``https://sourcegraph.com``,
the ``src`` CLI's convention) are read from ``os.environ`` at call time so a
session that sets them after import still works.  GraphQL ``errors`` payloads
surface as :class:`SourcegraphError` rather than silently returning ``None``.

The ``_client`` hook (see below) lets tests inject an ``httpx.MockTransport``
so every code path is exercisable with no network.
"""

from __future__ import annotations

import os
import re
from typing import TYPE_CHECKING, Any

import polars as pl

if TYPE_CHECKING:
    import httpx

__all__ = [
    "SourcegraphError",
    "search",
]

__version__ = "0.1.0"

_DEFAULT_ENDPOINT = "https://sourcegraph.com"

# Fixed schema so empty results stay typed (polars cannot infer a schema from
# zero rows) and downstream `.filter`/`.group_by` compose without surprises.
_SEARCH_SCHEMA: dict[str, pl.DataType | type[pl.DataType]] = {
    "kind": pl.Utf8,
    "repo": pl.Utf8,
    "stars": pl.Int64,
    "path": pl.Utf8,
    "line": pl.Int64,
    "content": pl.Utf8,
    "commit": pl.Utf8,
    "url": pl.Utf8,
}


class SourcegraphError(RuntimeError):
    """Raised when the Sourcegraph GraphQL response contains an ``errors`` field.

    The raw list of error dicts is available as ``.errors`` so callers can
    inspect codes/locations without parsing the exception message.
    """

    def __init__(self, errors: list[dict[str, Any]]) -> None:
        self.errors = errors
        msgs = "; ".join(str(e.get("message", e)) for e in errors)
        super().__init__(f"Sourcegraph API error: {msgs}")


def _endpoint() -> str:
    """The instance base URL: ``SRC_ENDPOINT`` or public sourcegraph.com."""
    return os.environ.get("SRC_ENDPOINT", "").rstrip("/") or _DEFAULT_ENDPOINT


# _client is module-level so tests can replace it with a factory that injects
# httpx.MockTransport without patching internals:
#
#   import sourcegraph, httpx
#   sourcegraph._client = lambda **kw: httpx.AsyncClient(
#       transport=httpx.MockTransport(handler), **kw
#   )
#
# Production code calls _client() each time so a token/endpoint set after
# import (common in notebooks) is always picked up.
def _client(**kwargs: Any) -> httpx.AsyncClient:  # noqa: ANN401 -- forwarded verbatim to httpx.AsyncClient
    """Return a fresh ``httpx.AsyncClient`` wired for the Sourcegraph API.

    Anonymous unless ``SRC_ACCESS_TOKEN`` is set (then ``Authorization:
    token <...>`` rides every request).  Keyword arguments are forwarded to
    the constructor, letting callers (and tests) override ``transport`` etc.
    """
    import httpx

    headers = {"Content-Type": "application/json"}
    token = os.environ.get("SRC_ACCESS_TOKEN", "")
    if token:
        headers["Authorization"] = f"token {token}"
    return httpx.AsyncClient(base_url=_endpoint(), headers=headers, **kwargs)


# Retry transient failures so an unattended sweep is not killed by a blip.
# Search is a read-only query, so replay is always safe (unlike linear's
# mutations). Scope: HTTP 5xx only; 4xx is a caller bug and must not retry.
_RETRY_BACKOFFS_S: tuple[float, ...] = (0.5, 1.5)

_SEARCH_QUERY = """
query IxSourcegraphSearch($query: String!) {
  search(query: $query, version: V3) {
    results {
      matchCount
      limitHit
      results {
        __typename
        ... on FileMatch {
          repository { name stars }
          file {
            path
            url
            ... on GitBlob { commit { oid } }
          }
          lineMatches { preview lineNumber }
        }
        ... on Repository {
          name
          stars
          description
          url
        }
        ... on CommitSearchResult {
          url
          commit {
            oid
            subject
            repository { name stars }
          }
        }
      }
    }
  }
}
"""


async def _gql(query: str, variables: dict[str, Any]) -> dict[str, Any]:
    """Execute one GraphQL operation and return the ``data`` dict.

    Transient HTTP 5xx failures are retried with backoff per
    :data:`_RETRY_BACKOFFS_S` plus one.  GraphQL errors raise
    :class:`SourcegraphError`; other HTTP errors raise
    ``httpx.HTTPStatusError``.
    """
    import asyncio

    payload = {"query": query, "variables": variables}
    total_attempts = len(_RETRY_BACKOFFS_S) + 1
    for attempt in range(total_attempts):
        last = attempt == total_attempts - 1
        async with _client() as client:
            resp = await client.post("/.api/graphql", json=payload)
            if 500 <= resp.status_code < 600 and not last:
                await asyncio.sleep(_RETRY_BACKOFFS_S[attempt])
                continue
            resp.raise_for_status()
            body: dict[str, Any] = resp.json()
        errors = body.get("errors")
        if errors:
            raise SourcegraphError(errors)
        data: dict[str, Any] = body.get("data") or {}
        return data
    raise RuntimeError("unreachable: _gql retry loop exited without return or raise")


# `count:` (with optional trailing value) already present in the query means the
# caller owns the result cap; `first=` must not append a second one.
_COUNT_RE = re.compile(r"(^|\s)count:\S*", re.IGNORECASE)


def _rows_from_file_match(result: dict[str, Any]) -> list[dict[str, Any]]:
    repository: dict[str, Any] = result.get("repository") or {}
    file: dict[str, Any] = result.get("file") or {}
    commit = (file.get("commit") or {}).get("oid")
    base = {
        "kind": "content",
        "repo": repository.get("name"),
        "stars": repository.get("stars"),
        "path": file.get("path"),
        "commit": commit,
        "url": file.get("url"),
    }
    line_matches: list[dict[str, Any]] = result.get("lineMatches") or []
    if not line_matches:
        # A path-only hit (`type:path` / `select:file`) still deserves a row.
        return [{**base, "line": None, "content": None}]
    return [
        {
            **base,
            # The GraphQL API's lineNumber is 0-based; editors and humans are
            # 1-based, so convert at the boundary.
            "line": lm["lineNumber"] + 1 if lm.get("lineNumber") is not None else None,
            "content": lm.get("preview"),
        }
        for lm in line_matches
    ]


def _row_from_repository(result: dict[str, Any]) -> dict[str, Any]:
    return {
        "kind": "repo",
        "repo": result.get("name"),
        "stars": result.get("stars"),
        "path": None,
        "line": None,
        "content": result.get("description"),
        "commit": None,
        "url": result.get("url"),
    }


def _row_from_commit(result: dict[str, Any]) -> dict[str, Any]:
    commit: dict[str, Any] = result.get("commit") or {}
    repository: dict[str, Any] = commit.get("repository") or {}
    return {
        "kind": "commit",
        "repo": repository.get("name"),
        "stars": repository.get("stars"),
        "path": None,
        "line": None,
        "content": commit.get("subject"),
        "commit": commit.get("oid"),
        "url": result.get("url"),
    }


async def search(query: str, *, first: int = 50) -> pl.DataFrame:
    """Search global public code on Sourcegraph; one polars row per match.

    ``query`` is a Sourcegraph search query
    (https://sourcegraph.com/docs/code-search/queries) -- plain terms plus
    filters like ``repo:``, ``lang:``, ``type:repo``, ``type:commit``,
    ``patterntype:regexp``.  ``first`` caps the result count by appending
    ``count:N`` unless the query already carries a ``count:`` filter.

    Returns a :class:`polars.DataFrame` with the fixed schema described in the
    module docstring (``kind``/``repo``/``stars``/``path``/``line``/``content``
    /``commit``/``url``); ``line`` is 1-based.  A content match yields one row
    per matched line, so a single file can span several rows -- aggregate with
    ``df.group_by("repo", "path")`` when you want per-file counts.

    Anonymous against public sourcegraph.com by default; set
    ``SRC_ACCESS_TOKEN`` (and optionally ``SRC_ENDPOINT``) for authenticated
    or private-instance search.  Raises :class:`SourcegraphError` on GraphQL
    errors and ``httpx.HTTPStatusError`` on HTTP errors.
    """
    effective = query if _COUNT_RE.search(query) else f"{query} count:{first}"
    data = await _gql(_SEARCH_QUERY, {"query": effective})
    results: list[dict[str, Any]] = ((data.get("search") or {}).get("results") or {}).get("results") or []

    rows: list[dict[str, Any]] = []
    for result in results:
        typename = result.get("__typename")
        if typename == "FileMatch":
            rows.extend(_rows_from_file_match(result))
        elif typename == "Repository":
            rows.append(_row_from_repository(result))
        elif typename == "CommitSearchResult":
            rows.append(_row_from_commit(result))
        # Unknown result types (future API additions) are skipped, not fatal.
    return pl.DataFrame(rows, schema=_SEARCH_SCHEMA)
