"""`watch_pr` must not arm auto merge on an already-mergeable PR (#2532).

On a repo with no blocking required checks, GitHub merges a PR the moment
`gh pr merge --auto` is armed, before any watching happens, so `watch_pr`
reads mergeStateStatus first and skips arming with a loud note instead.
"""

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest

from ix_notebook_mcp import runtime


class _ScriptedNu:
    """Stand-in for the bundled `nu` module: `watch_pr` does `import nu as
    nu_call` then calls the module object itself, so the TYPE must define
    `__call__`. Serves scripted `gh pr view` rows (the last row repeats) and
    records every `gh pr merge` invocation."""

    def __init__(
        self,
        views: list[dict[str, object]],
        *,
        required: list[list[dict[str, object]]] | None = None,
        merge_error: str | None = None,
    ) -> None:
        self._views = list(views)
        self._required = list(required or [[]])
        self._merge_error = merge_error
        self.merges: list[str] = []
        self.required_calls = 0

    async def __call__(
        self,
        code: str,
        *,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout: float = 60,
    ) -> dict[str, object]:
        if "gh pr merge" in code:
            self.merges.append(code)
            if self._merge_error is not None:
                return {"exit_code": 1, "stdout": "", "stderr": self._merge_error}
            return {"exit_code": 0, "stdout": "", "stderr": ""}
        if "gh pr checks" in code:
            assert "--required" in code
            self.required_calls += 1
            checks = (
                self._required.pop(0) if len(self._required) > 1 else self._required[0]
            )
            return {"exit_code": 0, "stdout": json.dumps(checks), "stderr": ""}
        assert "gh pr view" in code
        view = self._views.pop(0) if len(self._views) > 1 else self._views[0]
        return {"exit_code": 0, "stdout": json.dumps(view), "stderr": ""}


def _view(pr: int, state: str, merge_state: str) -> dict[str, object]:
    return {
        "number": pr,
        "title": "fix: guard",
        "state": state,
        "mergeStateStatus": merge_state,
        "statusCheckRollup": [],
        "url": f"https://github.com/o/r/pull/{pr}",
        "autoMergeRequest": None,
        "isDraft": False,
        "reviewDecision": "",
    }


def _watch(
    monkeypatch: pytest.MonkeyPatch, pr: int, views: list[dict[str, object]]
) -> tuple[dict[str, object], _ScriptedNu, list[str]]:
    fake = _ScriptedNu(views)
    monkeypatch.setitem(sys.modules, "nu", fake)
    notified: list[str] = []

    async def fake_notify(content: str, **meta: object) -> None:
        notified.append(content)

    monkeypatch.setattr(runtime, "notify", fake_notify)
    result = asyncio.run(runtime.watch_pr(pr, auto_merge=True, interval=0.01))
    return result, fake, notified


def test_already_mergeable_pr_skips_arming_with_loud_note(monkeypatch: pytest.MonkeyPatch) -> None:
    """CLEAN at arm time: no `gh pr merge --auto`, and the note reaches both
    the returned summary and the notify stream."""
    result, fake, notified = _watch(
        monkeypatch, 9101, [_view(9101, "OPEN", "CLEAN"), _view(9101, "MERGED", "CLEAN")]
    )
    assert fake.merges == []
    assert result["state"] == "MERGED"
    note = str(result["auto_merge"])
    assert "NOT armed" in note
    assert "gh pr merge 9101 --squash" in note
    assert any("NOT armed" in text for text in notified)


def test_unknown_mergeability_polls_until_it_resolves(monkeypatch: pytest.MonkeyPatch) -> None:
    """A fresh PR reports UNKNOWN while GitHub computes mergeability; the guard
    polls past it and still catches the instant-merge case (#2532's repro: the
    PR that merged 8 seconds after creation)."""
    monkeypatch.setattr(runtime, "_MERGEABILITY_POLL_S", 0.01)
    result, fake, _notified = _watch(
        monkeypatch,
        9102,
        [
            _view(9102, "OPEN", "UNKNOWN"),
            _view(9102, "OPEN", "CLEAN"),
            _view(9102, "MERGED", "CLEAN"),
        ],
    )
    assert fake.merges == []
    assert "NOT armed" in str(result["auto_merge"])


def test_blocked_pr_still_arms_auto_merge(monkeypatch: pytest.MonkeyPatch) -> None:
    """BLOCKED (required checks pending) keeps the old behavior: arm and watch."""
    result, fake, _notified = _watch(
        monkeypatch, 9103, [_view(9103, "OPEN", "BLOCKED"), _view(9103, "MERGED", "BLOCKED")]
    )
    assert len(fake.merges) == 1
    assert "--auto" in fake.merges[0]
    assert "auto_merge" not in result


@pytest.mark.parametrize(
    ("current_status", "current_conclusion", "current_state", "current_bucket"),
    [
        ("COMPLETED", "SUCCESS", "SUCCESS", "pass"),
        ("QUEUED", "", "QUEUED", "pending"),
    ],
)
def test_stale_cancelled_attempt_does_not_override_current_required_context(
    monkeypatch: pytest.MonkeyPatch,
    current_status: str,
    current_conclusion: str,
    current_state: str,
    current_bucket: str,
) -> None:
    first = _view(9108, "OPEN", "BLOCKED")
    attempts = [
        {
            "name": "flake-check",
            "workflowName": "Check",
            "status": "COMPLETED",
            "conclusion": "CANCELLED",
            "startedAt": "2026-07-15T07:08:36Z",
            "completedAt": "2026-07-15T07:13:46Z",
        },
        {
            "name": "flake-check",
            "workflowName": "Check",
            "status": current_status,
            "conclusion": current_conclusion,
            "startedAt": "2026-07-15T07:15:00Z",
            "completedAt": "2026-07-15T07:36:24Z" if current_conclusion else "",
        },
    ]
    first["statusCheckRollup"] = attempts
    merged = _view(9108, "MERGED", "CLEAN")
    merged["statusCheckRollup"] = attempts
    fake = _ScriptedNu(
        [first, first, merged],
        required=[
            [
                {
                    "name": "flake-check",
                    "workflow": "Check",
                    "state": current_state,
                    "bucket": current_bucket,
                    "startedAt": "2026-07-15T07:15:00Z",
                    "completedAt": "2026-07-15T07:36:24Z" if current_conclusion else "",
                }
            ]
        ],
    )
    monkeypatch.setitem(sys.modules, "nu", fake)
    notified: list[str] = []

    async def fake_notify(content: str, **meta: object) -> None:
        notified.append(content)

    monkeypatch.setattr(runtime, "notify", fake_notify)

    result = asyncio.run(runtime.watch_pr(9108, auto_merge=True, interval=0.01))

    assert result["state"] == "MERGED"
    assert fake.required_calls == 1
    assert not any("failing checks" in message for message in notified)
    html = asyncio.run(runtime.resources["pr-9108"].render_html())
    assert html.count("flake-check") == 2


class _FrameNu(_ScriptedNu):
    """A pre-#2394 nu build: a single record arrives as a 1-row polars
    DataFrame instead of a plain dict (#3175)."""

    async def __call__(self, code: str, **kwargs: object) -> object:
        import polars as pl

        value = await super().__call__(code, **kwargs)
        return pl.DataFrame([value])


class _TrackedResource:
    id = "pr-test"

    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True


def _capture_terminal_failure(
    monkeypatch: pytest.MonkeyPatch,
) -> tuple[_TrackedResource, list[str]]:
    resource = _TrackedResource()
    notifications: list[str] = []

    def register_resource(**kwargs: object) -> _TrackedResource:
        return resource

    async def fake_notify(content: str, **meta: object) -> None:
        notifications.append(content)

    monkeypatch.setattr(runtime, "register_resource", register_resource)
    monkeypatch.setattr(runtime, "notify", fake_notify)
    return resource, notifications


def test_one_row_frame_from_stale_nu_still_watches(monkeypatch: pytest.MonkeyPatch) -> None:
    """#3175: `refresh()` died with AttributeError DataFrame.get when nu
    returned a 1-row frame for the gh pr view record; the boundary now
    normalizes it and the watch survives to the terminal state."""
    fake = _FrameNu([_view(9104, "OPEN", "BLOCKED"), _view(9104, "MERGED", "BLOCKED")])
    monkeypatch.setitem(sys.modules, "nu", fake)

    async def fake_notify(content: str, **meta: object) -> None:
        pass

    monkeypatch.setattr(runtime, "notify", fake_notify)
    result = asyncio.run(runtime.watch_pr(9104, auto_merge=True, interval=0.01))
    assert result["state"] == "MERGED"
    assert len(fake.merges) == 1


def test_nu_record_rejects_unexpected_shapes() -> None:
    import polars as pl

    assert runtime._nu_record({"a": 1}, source="t") == {"a": 1}
    assert runtime._nu_record(pl.DataFrame([{"a": 1}]), source="t") == {"a": 1}
    with pytest.raises(TypeError, match="2-row frame"):
        runtime._nu_record(pl.DataFrame([{"a": 1}, {"a": 2}]), source="t")
    with pytest.raises(TypeError, match="expected a record"):
        runtime._nu_record("text", source="t")


def test_failed_view_preserves_completion_error(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """#2637: validate `complete` before parsing stdout, so a failed gh call
    keeps its real diagnostic instead of becoming an empty DataFrame."""
    gh = tmp_path / "gh"
    gh.write_text(
        "#!/bin/sh\nprintf '%s\\n' 'GraphQL: API rate limit already exceeded' >&2\nexit 1\n"
    )
    gh.chmod(0o755)
    monkeypatch.setenv("PATH", os.pathsep.join((str(tmp_path), os.environ["PATH"])))
    resource, notifications = _capture_terminal_failure(monkeypatch)
    import nu

    nu.reset()
    try:
        with pytest.raises(
            RuntimeError,
            match="gh pr view failed with exit code 1: GraphQL: API rate limit already exceeded",
        ):
            asyncio.run(runtime.watch_pr(9105, auto_merge=False))
    finally:
        nu.reset()

    assert resource.closed
    assert notifications == [
        "PR 9105 watch failed: gh pr view failed with exit code 1: "
        "GraphQL: API rate limit already exceeded"
    ]


def test_failed_auto_merge_is_terminal(monkeypatch: pytest.MonkeyPatch) -> None:
    fake = _ScriptedNu([_view(9106, "OPEN", "BLOCKED")], merge_error="merge denied")
    monkeypatch.setitem(sys.modules, "nu", fake)
    resource, notifications = _capture_terminal_failure(monkeypatch)

    with pytest.raises(
        RuntimeError, match="gh pr merge failed with exit code 1: merge denied"
    ):
        asyncio.run(runtime.watch_pr(9106, auto_merge=True, interval=0.01))

    assert resource.closed
    assert notifications == [
        "PR 9106 watch failed: gh pr merge failed with exit code 1: merge denied"
    ]


def test_notification_error_does_not_mask_command_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    fake = _ScriptedNu([_view(9107, "OPEN", "BLOCKED")], merge_error="merge denied")
    monkeypatch.setitem(sys.modules, "nu", fake)
    resource, _notifications = _capture_terminal_failure(monkeypatch)

    async def failing_notify(content: str, **meta: object) -> None:
        raise RuntimeError("outbox unavailable")

    monkeypatch.setattr(runtime, "notify", failing_notify)
    with pytest.raises(
        RuntimeError, match="gh pr merge failed with exit code 1: merge denied"
    ) as caught:
        asyncio.run(runtime.watch_pr(9107, auto_merge=True, interval=0.01))

    assert resource.closed
    assert caught.value.__notes__ == [
        "pr_watch failure notification also raised RuntimeError: outbox unavailable"
    ]
