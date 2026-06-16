"""Linear issue tracker over the GraphQL API using LINEAR_API_KEY.

Bundled like ``sh``/``tasks`` so every session can ``import linear`` and
interact with Linear without hand-rolling an httpx client each time.  When
``LINEAR_API_KEY`` is present in the environment the four operations agents
actually need work immediately with no additional setup:

    import linear

    # Read an issue
    issue = await linear.issue("ENG-123")
    issue["title"]               # string
    issue["state"]["name"]       # "In Progress", "Done", ...

    # Update an issue
    await linear.issue_update("ENG-123", stateId="<uuid>", priority=2)

    # Create an issue
    new = await linear.issue_create("ENG", "Fix the widget", description="...")
    new["id"]                    # the new issue's UUID

    # Create a project
    proj = await linear.project_create("Q3 Hardening", ["ENG"], description="...")
    proj["id"]

All four are async (kernel-loop style: no blocking network calls on the shared
event loop) and wrap the Linear GraphQL API
(https://developers.linear.app/docs/graphql/working-with-the-graphql-api).

``LINEAR_API_KEY`` is read from ``os.environ`` at call time so a session that
sets the key after import still works.  A missing key raises ``RuntimeError``
with a clear message.  GraphQL ``errors`` payloads surface as
:class:`LinearError` rather than silently returning ``None`` data fields.

The ``_client`` hook (see below) lets tests inject an ``httpx.MockTransport``
so every code path is exercisable with no network.
"""

from __future__ import annotations

import os
import re
from typing import Any

__all__ = [
    "issue",
    "issue_update",
    "issue_create",
    "issue_search",
    "comment_create",
    "project_create",
    "LinearError",
]

__version__ = "0.2.0"

_ENDPOINT = "https://api.linear.app/graphql"

# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


class LinearError(RuntimeError):
    """Raised when the Linear GraphQL response contains an ``errors`` field.

    The raw list of error dicts is available as ``.errors`` so callers can
    inspect extensions/codes without parsing the exception message.
    """

    def __init__(self, errors: list[dict[str, Any]]) -> None:
        self.errors = errors
        msgs = "; ".join(e.get("message", str(e)) for e in errors)
        super().__init__(f"Linear API error: {msgs}")


def _api_key() -> str:
    """Return the Linear API key from the environment or raise clearly."""
    key = os.environ.get("LINEAR_API_KEY", "")
    if not key:
        raise RuntimeError(
            "LINEAR_API_KEY is not set in the environment. "
            "Provision the key and retry."
        )
    return key


# _client is module-level so tests can replace it with a factory that
# injects httpx.MockTransport without patching internals:
#
#   import linear, httpx
#   linear._client = lambda **kw: httpx.AsyncClient(
#       transport=httpx.MockTransport(handler), **kw
#   )
#
# Production code calls _client() each time so that a key set after import
# (common in notebooks) is always picked up.
def _client(**kwargs: Any):  # noqa: ANN201
    """Return a fresh ``httpx.AsyncClient`` wired for the Linear GraphQL API.

    Keyword arguments are forwarded to the constructor, letting callers (and
    tests) override ``base_url``, ``transport``, etc.
    """
    import httpx

    key = _api_key()
    headers = {"Authorization": key, "Content-Type": "application/json"}
    return httpx.AsyncClient(headers=headers, **kwargs)


# Retry transient failures so an unattended cron sweep is not killed by a
# Linear blip. Scope: HTTP 5xx and the GraphQL "Internal server error" message
# observed in production -- both have been seen to recover on an immediate
# retry. 4xx and other GraphQL errors are caller bugs and must not retry.
#
# CRITICAL: retries are restricted to GraphQL *queries*. Mutations
# (issueCreate, issueUpdate, commentCreate, projectCreate) are not safe to
# replay -- Linear may have committed the write before the transient error,
# so a retry would produce a duplicate. The Linear API does not accept an
# idempotency key, so the only safe option is to fail fast and let the next
# triage pass dedup via the marker fingerprint.
_GQL_RETRY_BACKOFFS_S: tuple[float, ...] = (0.5, 1.5)


def _is_internal_server_error(errors: list[dict[str, Any]]) -> bool:
    return any(
        "internal server error" in str(e.get("message", "")).lower() for e in errors
    )


def _is_query(operation: str) -> bool:
    """True iff ``operation`` is a read-only GraphQL ``query`` (vs ``mutation``).

    All operations in this module are constants beginning with ``query`` or
    ``mutation`` after optional leading whitespace. Anything that does not
    parse cleanly as a ``query`` is treated as a mutation -- safe-by-default.
    """
    return operation.lstrip().startswith("query")


async def _gql(
    query: str,
    variables: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Execute one GraphQL operation and return the ``data`` dict.

    For ``query`` operations, transient failures (HTTP 5xx, GraphQL
    "Internal server error") are retried with exponential backoff up to
    :data:`_GQL_RETRY_BACKOFFS_S` plus one. ``mutation`` operations are not
    retried -- see the comment on :data:`_GQL_RETRY_BACKOFFS_S`. Any other
    failure raises immediately: :class:`LinearError` for GraphQL errors,
    ``httpx.HTTPStatusError`` for non-transient HTTP errors.
    """
    import asyncio

    import httpx

    payload: dict[str, Any] = {"query": query}
    if variables:
        payload["variables"] = variables

    total_attempts = len(_GQL_RETRY_BACKOFFS_S) + 1 if _is_query(query) else 1
    for attempt in range(total_attempts):
        last = attempt == total_attempts - 1
        async with _client() as client:
            resp = await client.post(_ENDPOINT, json=payload)
            if 500 <= resp.status_code < 600 and not last:
                await asyncio.sleep(_GQL_RETRY_BACKOFFS_S[attempt])
                continue
            resp.raise_for_status()
            body = resp.json()

        if body.get("errors"):
            if not last and _is_internal_server_error(body["errors"]):
                await asyncio.sleep(_GQL_RETRY_BACKOFFS_S[attempt])
                continue
            raise LinearError(body["errors"])
        return body.get("data", {})

    raise RuntimeError("unreachable: _gql retry loop exited without return or raise")


# A Linear UUID: 8-4-4-4-12 lowercase hex. Team keys ("ENG") never match this,
# so it is a safe, network-free way to tell "already a UUID" from "a key to
# resolve". Resolved keys are cached for the process lifetime -- a team's UUID
# never changes, and this keeps a fan-out of issue_create calls to one lookup.
_UUID_RE = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE
)
_team_id_cache: dict[str, str] = {}

_TEAM_BY_KEY_QUERY = """
query TeamByKey($key: String!) {
  teams(filter: { key: { eq: $key } }, first: 1) {
    nodes { id key }
  }
}
"""


async def _resolve_team_id(team: str) -> str:
    """Resolve a team key (e.g. ``"ENG"``) to its UUID; pass a UUID through as-is.

    Linear's ``IssueCreateInput.teamId`` and ``ProjectCreateInput.teamIds`` take
    team UUIDs, not the human-facing key, and reject a key with an opaque
    "Argument Validation Error". Callers naturally reach for the key, so resolve
    it here. Raises :class:`LinearError` if no team has that key.
    """
    if _UUID_RE.match(team):
        return team
    cached = _team_id_cache.get(team)
    if cached is not None:
        return cached
    data = await _gql(_TEAM_BY_KEY_QUERY, {"key": team})
    nodes = data["teams"]["nodes"]
    if not nodes:
        raise LinearError([{"message": f"no Linear team with key {team!r}"}])
    tid = nodes[0]["id"]
    _team_id_cache[team] = tid
    return tid


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------

_ISSUE_FIELDS = """
    id
    identifier
    title
    description
    priority
    url
    state { id name type }
    assignee { id name email }
    team { id name key }
    createdAt
    updatedAt
"""

_ISSUE_QUERY = f"""
query IssueById($id: String!) {{
  issue(id: $id) {{
    {_ISSUE_FIELDS}
  }}
}}
"""


async def issue(id: str) -> dict[str, Any]:
    """Fetch a Linear issue by its UUID or identifier (e.g. ``"ENG-123"``).

    Returns the issue as a plain dict with the fields:
    ``id``, ``identifier``, ``title``, ``description``, ``priority``,
    ``url``, ``state`` (``id``/``name``/``type``),
    ``assignee`` (``id``/``name``/``email`` or ``None``),
    ``team`` (``id``/``name``/``key``),
    ``createdAt``, ``updatedAt``.

    Raises :class:`LinearError` on GraphQL errors and
    ``httpx.HTTPStatusError`` on network errors.
    """
    data = await _gql(_ISSUE_QUERY, {"id": id})
    return data["issue"]


_ISSUE_UPDATE_MUTATION = """
mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue {
      id
      identifier
      title
      state { id name }
    }
  }
}
"""


async def issue_update(id: str, **fields: Any) -> dict[str, Any]:
    """Update fields on a Linear issue.

    ``id`` is the issue UUID or identifier (e.g. ``"ENG-123"``).
    Pass the fields you want to change as keyword arguments, using the names
    from the Linear ``IssueUpdateInput`` type, for example::

        await linear.issue_update("ENG-123", stateId="<uuid>", priority=2)
        await linear.issue_update("ENG-123", title="Better title",
                                  description="More detail")

    Returns the updated issue dict (``id``, ``identifier``, ``title``,
    ``state``).  Raises :class:`LinearError` on GraphQL errors.
    """
    data = await _gql(_ISSUE_UPDATE_MUTATION, {"id": id, "input": fields})
    return data["issueUpdate"]["issue"]


_ISSUE_CREATE_MUTATION = """
mutation IssueCreate($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue {
      id
      identifier
      title
      url
      state { id name }
      team { id name key }
    }
  }
}
"""


async def issue_create(team: str, title: str, **fields: Any) -> dict[str, Any]:
    """Create a new Linear issue.

    ``team`` is the team key (e.g. ``"ENG"``) or UUID.
    ``title`` is required.  Any additional ``IssueCreateInput`` fields can be
    passed as keyword arguments::

        new = await linear.issue_create("ENG", "Fix the widget",
                                        description="Details here",
                                        priority=2)
        new["identifier"]   # "ENG-456"
        new["url"]          # "https://linear.app/..."

    Returns the created issue dict.  Raises :class:`LinearError` on errors.
    """
    input_vars: dict[str, Any] = {
        "teamId": await _resolve_team_id(team),
        "title": title,
        **fields,
    }
    data = await _gql(_ISSUE_CREATE_MUTATION, {"input": input_vars})
    return data["issueCreate"]["issue"]


_PROJECT_CREATE_MUTATION = """
mutation ProjectCreate($input: ProjectCreateInput!) {
  projectCreate(input: $input) {
    success
    project {
      id
      name
      url
      state
      teams { nodes { id name key } }
    }
  }
}
"""


async def project_create(
    name: str,
    teams: list[str],
    **fields: Any,
) -> dict[str, Any]:
    """Create a new Linear project.

    ``name`` is the project name.
    ``teams`` is a list of team keys or UUIDs to associate with the project.
    Any additional ``ProjectCreateInput`` fields can be passed as keyword
    arguments::

        proj = await linear.project_create(
            "Q3 Hardening",
            ["ENG"],
            description="Reliability push for Q3",
        )
        proj["id"]
        proj["url"]

    Returns the created project dict.  Raises :class:`LinearError` on errors.
    """
    input_vars: dict[str, Any] = {
        "name": name,
        "teamIds": [await _resolve_team_id(t) for t in teams],
        **fields,
    }
    data = await _gql(_PROJECT_CREATE_MUTATION, {"input": input_vars})
    return data["projectCreate"]["project"]


_ISSUE_SEARCH_QUERY = """
query IssueSearch($term: String!, $first: Int!) {
  searchIssues(term: $term, first: $first) {
    nodes {
      id
      identifier
      title
      url
      description
      state { id name type }
    }
  }
}
"""


async def issue_search(term: str, first: int = 20) -> list[dict[str, Any]]:
    """Search Linear issues by keyword or phrase.

    ``term`` is the search string forwarded to Linear's full-text search.
    ``first`` caps the number of results returned (default 20).

    Returns a list of issue dicts, each containing at least
    ``id``, ``identifier``, ``title``, ``url``, ``description``,
    and ``state`` (``id``/``name``/``type``).

    Raises :class:`LinearError` on GraphQL errors and
    ``httpx.HTTPStatusError`` on network errors.
    """
    data = await _gql(_ISSUE_SEARCH_QUERY, {"term": term, "first": first})
    return data["searchIssues"]["nodes"]


_COMMENT_CREATE_MUTATION = """
mutation CommentCreate($input: CommentCreateInput!) {
  commentCreate(input: $input) {
    success
    comment {
      id
      url
    }
  }
}
"""


async def comment_create(issue_id: str, body: str) -> dict[str, Any]:
    """Add a comment to a Linear issue.

    ``issue_id`` is the issue UUID.
    ``body`` is the comment text (Markdown supported).

    Returns the created comment dict with ``id`` and ``url``.
    Raises :class:`LinearError` on GraphQL errors and
    ``httpx.HTTPStatusError`` on network errors.
    """
    input_vars: dict[str, Any] = {"issueId": issue_id, "body": body}
    data = await _gql(_COMMENT_CREATE_MUTATION, {"input": input_vars})
    return data["commentCreate"]["comment"]
