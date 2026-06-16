"""Unit tests for the linear.triage module and linear.__init__ additions.

Run with::

    PYTHONPATH=packages/mcp/src/linear pytest packages/mcp/tests/test_linear_triage.py -q

No network is used.  All Linear I/O goes through FakeLinearPort.
"""

from __future__ import annotations

import asyncio
import sys
import os

# Make `import linear` work when running directly against the source tree.
# In the nix env the module is installed; when running locally we add the src.
_src = os.path.join(os.path.dirname(__file__), "..", "src", "linear")
if _src not in sys.path:
    sys.path.insert(0, _src)

from typing import Any

import pytest

# Both import paths must work.
import linear  # noqa: E402
from linear import triage as triage_mod  # noqa: E402
from linear.triage import (  # noqa: E402
    Finding,
    TriageConfig,
    TriageResult,
    LinearPort,
    fingerprint,
    marker_line,
    triage,
    MARKER_KEY,
    _normalize,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


class FakeLinearPort:
    """In-memory LinearPort for tests.

    ``search_results`` maps a search term to the list of issue dicts returned.
    Calls to ``search``, ``create``, and ``comment`` are recorded in the
    corresponding lists so assertions can inspect them.
    """

    def __init__(self) -> None:
        self.search_results: dict[str, list[dict[str, Any]]] = {}
        self.created: list[dict[str, Any]] = []
        self.commented: list[tuple[str, str]] = []
        self._next_id = 1

    def _issue_id(self) -> str:
        uid = f"issue-{self._next_id:04d}"
        self._next_id += 1
        return uid

    async def search(self, term: str) -> list[dict[str, Any]]:
        return list(self.search_results.get(term, []))

    async def create(
        self,
        *,
        team_id: str,
        title: str,
        description: str,
        parent_id: str,
        label_ids: list[str],
        priority: int,
    ) -> dict[str, Any]:
        issue: dict[str, Any] = {
            "id": self._issue_id(),
            "title": title,
            "description": description,
            "identifier": f"ENG-{self._next_id}",
            "state": {"id": "state-todo", "name": "Todo", "type": "unstarted"},
        }
        self.created.append(issue)
        return issue

    async def comment(self, issue_id: str, body: str) -> dict[str, Any]:
        comment = {"id": f"comment-{len(self.commented) + 1}", "url": "#"}
        self.commented.append((issue_id, body))
        return comment


def _cfg(**overrides: Any) -> TriageConfig:
    defaults: dict[str, Any] = {
        "team_id": "team-uuid",
        "epic_id": "epic-uuid",
        "label_ids": ("label-uuid",),
        "max_new_per_run": 10,
    }
    defaults.update(overrides)
    return TriageConfig(**defaults)


def _finding(key: str = "key-1", priority: int = 3, **overrides: Any) -> Finding:
    defaults: dict[str, Any] = {
        "source": "ci",
        "kind": "lint",
        "key": key,
        "title": f"Finding {key}",
        "body_md": f"Description for {key}",
        "priority": priority,
    }
    defaults.update(overrides)
    return Finding(**defaults)


def run(coro):
    """Run an async coroutine synchronously for pytest compatibility."""
    return asyncio.run(coro)


# ---------------------------------------------------------------------------
# fingerprint stability
# ---------------------------------------------------------------------------


class TestFingerprintStability:
    def test_nix_store_hash_differences_same_fp(self):
        """Two findings whose key/body differ only by nix store hashes are equal."""
        f1 = Finding(
            source="ci",
            kind="lint",
            key="/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo/bar.rs",
            title="Lint failure",
            body_md="Error in /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo/bar.rs",
        )
        f2 = Finding(
            source="ci",
            kind="lint",
            key="/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-foo/bar.rs",
            title="Lint failure",
            body_md="Error in /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-foo/bar.rs",
        )
        assert fingerprint(f1) == fingerprint(f2)

    def test_nox_conformance_store_pid_same_fp(self):
        """Findings differing only by nox-conformance-store-<pid> yield same fp."""
        f1 = Finding(
            source="nox",
            kind="conformance",
            key="nox-conformance-store-12345/test.rs",
            title="Conformance failure",
            body_md="Failure in nox-conformance-store-12345",
        )
        f2 = Finding(
            source="nox",
            kind="conformance",
            key="nox-conformance-store-99999/test.rs",
            title="Conformance failure",
            body_md="Failure in nox-conformance-store-99999",
        )
        assert fingerprint(f1) == fingerprint(f2)

    def test_line_col_differences_same_fp(self):
        """Findings differing only by :line:col positions yield the same fp."""
        f1 = Finding(
            source="ci",
            kind="lint",
            key="src/foo.rs:10:5",
            title="Lint failure",
            body_md="Error at src/foo.rs:10:5",
        )
        f2 = Finding(
            source="ci",
            kind="lint",
            key="src/foo.rs:99:1",
            title="Lint failure",
            body_md="Error at src/foo.rs:99:1",
        )
        assert fingerprint(f1) == fingerprint(f2)

    def test_pid_differences_same_fp(self):
        """Findings differing only by pid numbers yield the same fp."""
        f1 = Finding(
            source="ci",
            kind="crash",
            key="crash in pid 1234",
            title="Crash",
            body_md="Process crash: pid 1234 exited unexpectedly",
        )
        f2 = Finding(
            source="ci",
            kind="crash",
            key="crash in pid 5678",
            title="Crash",
            body_md="Process crash: pid 5678 exited unexpectedly",
        )
        assert fingerprint(f1) == fingerprint(f2)

    def test_tmp_path_differences_same_fp(self):
        """Findings differing only by /tmp paths yield the same fp."""
        f1 = Finding(
            source="ci",
            kind="test",
            key="/tmp/run-abc123/output",
            title="Test failure",
            body_md="Output at /tmp/run-abc123/output",
        )
        f2 = Finding(
            source="ci",
            kind="test",
            key="/tmp/run-xyz789/output",
            title="Test failure",
            body_md="Output at /tmp/run-xyz789/output",
        )
        assert fingerprint(f1) == fingerprint(f2)

    def test_different_source_different_fp(self):
        """Findings from different sources yield different fingerprints."""
        f1 = _finding(source="ci")
        f2 = Finding(
            source="antithesis",
            kind=f1.kind,
            key=f1.key,
            title=f1.title,
            body_md=f1.body_md,
        )
        assert fingerprint(f1) != fingerprint(f2)

    def test_different_kind_different_fp(self):
        """Findings with different kinds yield different fingerprints."""
        f1 = _finding(kind="lint")
        f2 = Finding(
            source=f1.source,
            kind="crash",
            key=f1.key,
            title=f1.title,
            body_md=f1.body_md,
        )
        assert fingerprint(f1) != fingerprint(f2)

    def test_different_key_different_fp(self):
        """Findings with meaningfully different keys yield different fps."""
        f1 = _finding(key="attr-a::test_foo")
        f2 = _finding(key="attr-b::test_bar")
        assert fingerprint(f1) != fingerprint(f2)

    def test_fp_is_16_hex_chars(self):
        """fingerprint() returns exactly 16 hex characters."""
        fp = fingerprint(_finding())
        assert len(fp) == 16
        assert all(c in "0123456789abcdef" for c in fp)

    def test_normalize_idempotent(self):
        """_normalize applied twice yields the same result as once."""
        s = "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1-foo/bar:10:5 pid 42"
        assert _normalize(_normalize(s)) == _normalize(s)


# ---------------------------------------------------------------------------
# marker_line
# ---------------------------------------------------------------------------


class TestMarkerLine:
    def test_format(self):
        """marker_line returns the expected format."""
        fp = "deadbeef01234567"
        assert marker_line(fp) == f"{MARKER_KEY}: {fp}"

    def test_marker_key_constant(self):
        assert MARKER_KEY == "nox-fingerprint"


# ---------------------------------------------------------------------------
# Idempotency
# ---------------------------------------------------------------------------


class TestIdempotency:
    def test_first_pass_creates_issues(self):
        """First triage pass over N fresh findings (no existing issues) creates N issues."""
        port = FakeLinearPort()
        findings = [_finding(key=f"k{i}") for i in range(3)]
        result = run(triage(findings, _cfg(), port, dry_run=False))
        assert len(result.filed) == 3
        assert result.updated == []
        assert result.deferred == 0
        assert len(port.created) == 3

    def test_second_pass_bumps_not_creates(self):
        """Second pass where search returns the already-created issues yields 0 creates."""
        port = FakeLinearPort()
        findings = [_finding(key=f"k{i}") for i in range(3)]

        # First pass.
        run(triage(findings, _cfg(), port, dry_run=False))
        assert len(port.created) == 3

        # Inject the created issues into the fake port so the second pass finds them.
        # triage() searches by the bare 16-hex fingerprint, so key results by fp.
        for created_issue in port.created:
            desc = created_issue["description"]
            for line in desc.splitlines():
                if line.startswith(MARKER_KEY):
                    fp = line.strip().split(": ", 1)[1]
                    port.search_results[fp] = [created_issue]

        # Second pass.
        result2 = run(triage(findings, _cfg(), port, dry_run=False))
        assert result2.filed == []
        assert len(result2.updated) == 3
        # No new issues created.
        assert len(port.created) == 3
        # Bump comments posted.
        assert len(port.commented) == 3

    def test_dry_run_no_api_calls(self):
        """dry_run=True makes decisions but performs no create/comment calls."""
        port = FakeLinearPort()
        findings = [_finding(key="k1"), _finding(key="k2")]
        result = run(triage(findings, _cfg(), port, dry_run=True))
        assert len(result.filed) == 2
        assert result.dry_run is True
        assert port.created == []
        assert port.commented == []


# ---------------------------------------------------------------------------
# Cap and ordering
# ---------------------------------------------------------------------------


class TestCapAndOrdering:
    def test_max_new_per_run_respected(self):
        """With max_new_per_run=3 and 5 findings, exactly 3 are created."""
        port = FakeLinearPort()
        findings = [_finding(key=f"k{i}", priority=i % 4 + 1) for i in range(5)]
        cfg = _cfg(max_new_per_run=3)
        result = run(triage(findings, cfg, port, dry_run=False))
        assert len(result.filed) == 3
        assert result.deferred == 2
        assert len(port.created) == 3

    def test_most_urgent_created_first(self):
        """The 3 most urgent findings (lowest priority int, excluding 0) are filed."""
        port = FakeLinearPort()
        findings = [
            _finding(key="low", priority=4),
            _finding(key="medium", priority=3),
            _finding(key="high", priority=2),
            _finding(key="urgent", priority=1),
            _finding(key="none", priority=0),
        ]
        cfg = _cfg(max_new_per_run=3)
        result = run(triage(findings, cfg, port, dry_run=False))
        assert result.deferred == 2
        filed_titles = {c["title"] for c in port.created}
        # Urgent (1), High (2), Medium (3) should be created; Low (4) and None (0) deferred.
        assert "Finding urgent" in filed_titles
        assert "Finding high" in filed_titles
        assert "Finding medium" in filed_titles
        assert "Finding low" not in filed_titles
        assert "Finding none" not in filed_titles

    def test_priority_zero_is_lowest_urgency(self):
        """priority=0 is treated as lower urgency than priority=4."""
        port = FakeLinearPort()
        findings = [
            _finding(key="p4", priority=4),
            _finding(key="p0", priority=0),
            _finding(key="p1", priority=1),
        ]
        cfg = _cfg(max_new_per_run=2)
        result = run(triage(findings, cfg, port, dry_run=False))
        filed_titles = {c["title"] for c in port.created}
        assert "Finding p1" in filed_titles
        assert "Finding p4" in filed_titles
        assert "Finding p0" not in filed_titles
        assert result.deferred == 1


# ---------------------------------------------------------------------------
# Exact-title dedup guard
# ---------------------------------------------------------------------------


class TestExactTitleGuard:
    def test_exact_title_match_bumps_instead_of_creating(self):
        """A finding whose title matches an existing issue is bumped, not duplicated."""
        port = FakeLinearPort()
        existing = {
            "id": "existing-id",
            "title": "Finding k1",
            "description": "No marker here",
            "state": {"id": "s1", "name": "Todo", "type": "unstarted"},
        }
        f = _finding(key="k1")
        port.search_results[f.title] = [existing]
        result = run(triage([f], _cfg(), port, dry_run=False))
        assert result.filed == []
        assert result.updated == ["k1"]
        assert len(port.created) == 0
        assert len(port.commented) == 1


# ---------------------------------------------------------------------------
# Resolved (closed) issue re-opens as regression
# ---------------------------------------------------------------------------


class TestResolvedIssue:
    def test_closed_issue_gets_regression_comment(self):
        """A finding matching a completed issue gets a regression comment."""
        port = FakeLinearPort()
        f = _finding(key="k1")
        fp = fingerprint(f)
        marker = marker_line(fp)
        closed_issue = {
            "id": "closed-id",
            "title": f.title,
            "description": f"Some body\n\n{marker}",
            "state": {"id": "s-done", "name": "Done", "type": "completed"},
        }
        # triage() searches by the bare fingerprint, not the full marker line.
        port.search_results[fp] = [closed_issue]
        result = run(triage([f], _cfg(), port, dry_run=False))
        assert result.updated == ["k1"]
        assert len(port.commented) == 1
        comment_body = port.commented[0][1]
        assert "egression" in comment_body


# ---------------------------------------------------------------------------
# Module-level additions: issue_search and comment_create are callable
# ---------------------------------------------------------------------------


class TestLinearModuleAdditions:
    def test_issue_search_in_all(self):
        assert "issue_search" in linear.__all__

    def test_comment_create_in_all(self):
        assert "comment_create" in linear.__all__

    def test_version_bumped(self):
        assert linear.__version__ == "0.2.0"

    def test_issue_search_callable(self):
        assert callable(linear.issue_search)

    def test_comment_create_callable(self):
        assert callable(linear.comment_create)


# ---------------------------------------------------------------------------
# issue_search wire test (httpx.MockTransport)
# ---------------------------------------------------------------------------


class TestIssueSearchWire:
    def test_issue_search_posts_searchissues_query(self):
        """issue_search posts a searchIssues query and returns nodes."""
        import json
        import httpx

        nodes = [
            {
                "id": "abc",
                "identifier": "ENG-1",
                "title": "Test issue",
                "url": "https://linear.app/test",
                "description": "desc",
                "state": {"id": "s1", "name": "Todo", "type": "unstarted"},
            }
        ]
        response_body = {"data": {"searchIssues": {"nodes": nodes}}}
        received: list[dict] = []

        def handler(request: httpx.Request) -> httpx.Response:
            body = json.loads(request.content)
            received.append(body)
            return httpx.Response(200, json=response_body)

        original_client = linear._client
        linear._client = lambda **kw: httpx.AsyncClient(
            transport=httpx.MockTransport(handler), **kw
        )
        # Patch _api_key so it doesn't require LINEAR_API_KEY in env.
        original_api_key = linear._api_key
        linear._api_key = lambda: "test-key"

        try:
            result = run(linear.issue_search("some term", first=5))
        finally:
            linear._client = original_client
            linear._api_key = original_api_key

        assert result == nodes
        assert len(received) == 1
        assert "searchIssues" in received[0]["query"]
        assert received[0]["variables"] == {"term": "some term", "first": 5}


# ---------------------------------------------------------------------------
# Team key -> UUID resolution (issue_create / project_create)
# ---------------------------------------------------------------------------


class TestTeamResolution:
    """issue_create/project_create must send a team UUID, resolving a key first.

    Linear's IssueCreateInput.teamId / ProjectCreateInput.teamIds reject a
    human key like "ENG" with an opaque "Argument Validation Error", so the
    module resolves key -> UUID via a TeamByKey query (cached) before mutating.
    """

    @staticmethod
    def _wire(handler):
        """Install a MockTransport handler + fake api key; return a restore fn."""
        import httpx

        orig_client, orig_key = linear._client, linear._api_key
        linear._client = lambda **kw: httpx.AsyncClient(
            transport=httpx.MockTransport(handler), **kw
        )
        linear._api_key = lambda: "test-key"
        linear._team_id_cache.clear()

        def restore():
            linear._client, linear._api_key = orig_client, orig_key
            linear._team_id_cache.clear()

        return restore

    def test_key_resolved_to_uuid_and_cached(self):
        """A key triggers one TeamByKey lookup; the UUID is reused on later calls."""
        import json
        import httpx

        received: list[dict] = []

        def handler(request: httpx.Request) -> httpx.Response:
            body = json.loads(request.content)
            received.append(body)
            if "TeamByKey" in body["query"]:
                return httpx.Response(
                    200,
                    json={"data": {"teams": {"nodes": [{"id": "team-uuid-eng", "key": "ENG"}]}}},
                )
            return httpx.Response(
                200,
                json={"data": {"issueCreate": {"success": True, "issue": {"id": "i1", "identifier": "ENG-1", "title": "t", "url": "u", "state": None, "team": None}}}},
            )

        restore = self._wire(handler)
        try:
            run(linear.issue_create("ENG", "first"))
            run(linear.issue_create("ENG", "second"))
        finally:
            restore()

        lookups = [b for b in received if "TeamByKey" in b["query"]]
        teamids = [b["variables"]["input"]["teamId"] for b in received if "IssueCreate" in b["query"]]
        assert len(lookups) == 1, "key resolution must be cached, not re-queried"
        assert teamids == ["team-uuid-eng", "team-uuid-eng"]

    def test_uuid_passes_through_without_lookup(self):
        """A teamId already in UUID form is sent as-is, no TeamByKey query."""
        import json
        import httpx

        uuid = "550e8400-e29b-41d4-a716-446655440000"
        received: list[dict] = []

        def handler(request: httpx.Request) -> httpx.Response:
            body = json.loads(request.content)
            received.append(body)
            return httpx.Response(
                200,
                json={"data": {"issueCreate": {"success": True, "issue": {"id": "i1", "identifier": "ENG-1", "title": "t", "url": "u", "state": None, "team": None}}}},
            )

        restore = self._wire(handler)
        try:
            run(linear.issue_create(uuid, "x"))
        finally:
            restore()

        assert not any("TeamByKey" in b["query"] for b in received)
        teamids = [b["variables"]["input"]["teamId"] for b in received if "IssueCreate" in b["query"]]
        assert teamids == [uuid]

    def test_unknown_key_raises_linear_error(self):
        """A key with no matching team surfaces a clear LinearError."""
        import httpx

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, json={"data": {"teams": {"nodes": []}}})

        restore = self._wire(handler)
        try:
            with pytest.raises(linear.LinearError):
                run(linear.issue_create("NOPE", "x"))
        finally:
            restore()

    def test_project_create_resolves_each_team(self):
        """project_create resolves every team key in its list to a UUID."""
        import json
        import httpx

        received: list[dict] = []

        def handler(request: httpx.Request) -> httpx.Response:
            body = json.loads(request.content)
            received.append(body)
            if "TeamByKey" in body["query"]:
                key = body["variables"]["key"]
                return httpx.Response(
                    200,
                    json={"data": {"teams": {"nodes": [{"id": f"uuid-{key}", "key": key}]}}},
                )
            return httpx.Response(
                200,
                json={"data": {"projectCreate": {"success": True, "project": {"id": "p1", "name": "n", "url": "u", "state": None, "teams": {"nodes": []}}}}},
            )

        restore = self._wire(handler)
        try:
            run(linear.project_create("Proj", ["ENG", "ADM"]))
        finally:
            restore()

        team_ids = [b["variables"]["input"]["teamIds"] for b in received if "ProjectCreate" in b["query"]]
        assert team_ids == [["uuid-ENG", "uuid-ADM"]]


# ---------------------------------------------------------------------------
# _gql transient-failure retry
# ---------------------------------------------------------------------------


class TestGqlRetry:
    """Transient Linear failures must not kill an unattended triage run.

    Scope: HTTP 5xx and the GraphQL ``"Internal server error"`` message.
    Everything else (4xx, other GraphQL errors) must raise immediately.
    """

    @staticmethod
    def _wire(handler, *, sleep_calls: list[float] | None = None):
        """Install MockTransport, fake api key, and stub asyncio.sleep."""
        import httpx

        orig_client, orig_key = linear._client, linear._api_key
        linear._client = lambda **kw: httpx.AsyncClient(
            transport=httpx.MockTransport(handler), **kw
        )
        linear._api_key = lambda: "test-key"

        # Stub sleep so tests do not actually wait the backoff.
        import asyncio as _asyncio

        orig_sleep = _asyncio.sleep

        async def fake_sleep(s: float) -> None:
            if sleep_calls is not None:
                sleep_calls.append(s)

        _asyncio.sleep = fake_sleep  # type: ignore[assignment]

        def restore() -> None:
            linear._client, linear._api_key = orig_client, orig_key
            _asyncio.sleep = orig_sleep  # type: ignore[assignment]

        return restore

    def test_retries_on_transient_5xx(self):
        """A 500 followed by 200 succeeds and is observable as a single retry."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            if calls["n"] == 1:
                return httpx.Response(500, text="Internal Server Error")
            return httpx.Response(
                200, json={"data": {"searchIssues": {"nodes": []}}}
            )

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            result = run(linear.issue_search("t"))
        finally:
            restore()

        assert result == []
        assert calls["n"] == 2
        assert sleeps == [0.5]

    def test_retries_on_graphql_internal_server_error(self):
        """A GraphQL ``Internal server error`` is retried and then succeeds."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            if calls["n"] == 1:
                return httpx.Response(
                    200, json={"errors": [{"message": "Internal server error"}]}
                )
            return httpx.Response(
                200, json={"data": {"searchIssues": {"nodes": []}}}
            )

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            result = run(linear.issue_search("t"))
        finally:
            restore()

        assert result == []
        assert calls["n"] == 2
        assert sleeps == [0.5]

    def test_exhausts_retries_then_raises(self):
        """Three consecutive 500s exhaust the retry budget and raise."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(500, text="Internal Server Error")

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            with pytest.raises(httpx.HTTPStatusError):
                run(linear.issue_search("t"))
        finally:
            restore()

        assert calls["n"] == 3
        assert sleeps == [0.5, 1.5]

    def test_does_not_retry_on_4xx(self):
        """4xx is a caller bug and must raise on the first attempt."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(400, text="Bad Request")

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            with pytest.raises(httpx.HTTPStatusError):
                run(linear.issue_search("t"))
        finally:
            restore()

        assert calls["n"] == 1
        assert sleeps == []

    def test_does_not_retry_on_other_graphql_errors(self):
        """A non-transient GraphQL error must surface immediately as LinearError."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(
                200,
                json={"errors": [{"message": "Argument 'term' is invalid"}]},
            )

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            with pytest.raises(linear.LinearError):
                run(linear.issue_search("t"))
        finally:
            restore()

        assert calls["n"] == 1
        assert sleeps == []

    def test_mutations_do_not_retry_on_5xx(self):
        """Mutations must fail fast on 5xx -- the server may have committed
        the write, so a retry would duplicate (no idempotency key in the API)."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(500, text="Internal Server Error")

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            with pytest.raises(httpx.HTTPStatusError):
                run(linear.comment_create("issue-uuid", "hello"))
        finally:
            restore()

        assert calls["n"] == 1
        assert sleeps == []

    def test_mutations_do_not_retry_on_internal_server_error(self):
        """Mutations must fail fast on GraphQL 'Internal server error' too --
        the write may have committed before the error was returned."""
        import httpx

        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(
                200, json={"errors": [{"message": "Internal server error"}]}
            )

        sleeps: list[float] = []
        restore = self._wire(handler, sleep_calls=sleeps)
        try:
            with pytest.raises(linear.LinearError):
                run(linear.comment_create("issue-uuid", "hello"))
        finally:
            restore()

        assert calls["n"] == 1
        assert sleeps == []


# ---------------------------------------------------------------------------
# Dedup search uses the bare fingerprint, not the full marker line
# ---------------------------------------------------------------------------


class TestDedupSearchTerm:
    def test_search_is_called_with_bare_fingerprint(self):
        """``triage`` must search the 16-hex fingerprint to ride above Linear's
        fuzzy ranking; a hit keyed only by the bare fingerprint must dedup."""
        port = FakeLinearPort()
        f = _finding(key="k1")
        fp = fingerprint(f)
        marker = marker_line(fp)
        existing = {
            "id": "existing-id",
            "title": f.title,
            "description": f"Body\n\n{marker}",
            "state": {"id": "s1", "name": "Todo", "type": "unstarted"},
        }
        # Keyed by the bare fingerprint, NOT the full marker line.
        port.search_results[fp] = [existing]

        result = run(triage([f], _cfg(), port, dry_run=False))

        assert result.filed == []
        assert result.updated == ["k1"]
        assert len(port.created) == 0
