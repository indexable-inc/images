"""`watch_pr` must not arm auto merge on an already-mergeable PR (#2532).

On a repo with no blocking required checks, GitHub merges a PR the moment
`gh pr merge --auto` is armed, before any watching happens, so `watch_pr`
reads mergeStateStatus first and skips arming with a loud note instead.
"""

import asyncio
import sys

import pytest

from ix_notebook_mcp import runtime


class _ScriptedNu:
    """Stand-in for the bundled `nu` module: `watch_pr` does `import nu as
    nu_call` then calls the module object itself, so the TYPE must define
    `__call__`. Serves scripted `gh pr view` rows (the last row repeats) and
    records every `gh pr merge` invocation."""

    def __init__(self, views: list[dict[str, object]]) -> None:
        self._views = list(views)
        self.merges: list[str] = []

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
            return {"exit_code": 0, "stdout": "", "stderr": ""}
        assert "gh pr view" in code
        return self._views.pop(0) if len(self._views) > 1 else self._views[0]


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
