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
from typing import Any

__all__ = [
    "issue",
    "issue_update",
    "issue_create",
    "project_create",
    "LinearError",
]

__version__ = "0.1.0"

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


async def _gql(
    query: str,
    variables: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Execute one GraphQL operation and return the ``data`` dict.

    Raises :class:`LinearError` if the response contains ``errors``.
    """
    import httpx

    payload: dict[str, Any] = {"query": query}
    if variables:
        payload["variables"] = variables

    async with _client() as client:
        resp = await client.post(_ENDPOINT, json=payload)
        resp.raise_for_status()
        body = resp.json()

    if body.get("errors"):
        raise LinearError(body["errors"])
    return body.get("data", {})


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
    input_vars: dict[str, Any] = {"teamId": team, "title": title, **fields}
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
    input_vars: dict[str, Any] = {"name": name, "teamIds": teams, **fields}
    data = await _gql(_PROJECT_CREATE_MUTATION, {"input": input_vars})
    return data["projectCreate"]["project"]
