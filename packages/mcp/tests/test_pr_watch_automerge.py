"""`watch_pr` auto-merge and required-check terminal contracts (#2532, #3054).

On a repo with no blocking required checks, GitHub merges a PR the moment
`gh pr merge --auto` is armed, before any watching happens, so `watch_pr`
reads mergeStateStatus first and skips arming with a loud note instead.
"""

import asyncio
import json
import sys

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
        required: list[list[dict[str, object]]] | None = None,
        *,
        no_required: bool = False,
    ) -> None:
        self._views = list(views)
        self._required = list(required or [[]])
        self._no_required = no_required
        self.merges: list[str] = []
        self.view_calls = 0

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
        if "gh pr checks" in code:
            if self._no_required:
                return {
                    "exit_code": 1,
                    "stdout": "",
                    "stderr": "no required checks reported on the 'feature' branch",
                }
            checks = self._required.pop(0) if len(self._required) > 1 else self._required[0]
            if any(check.get("bucket") == "pending" for check in checks):
                exit_code = 8
            elif any(check.get("bucket") in {"fail", "cancel"} for check in checks):
                exit_code = 1
            else:
                exit_code = 0
            return {"exit_code": exit_code, "stdout": json.dumps(checks), "stderr": ""}
        assert "gh pr view" in code
        self.view_calls += 1
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


def _check(name: str, state: str, bucket: str) -> dict[str, object]:
    return {"name": name, "state": state, "bucket": bucket}


def _watch(
    monkeypatch: pytest.MonkeyPatch,
    pr: int,
    views: list[dict[str, object]],
    required: list[list[dict[str, object]]] | None = None,
    *,
    no_required: bool = False,
) -> tuple[dict[str, object], _ScriptedNu, list[str]]:
    fake = _ScriptedNu(views, required, no_required=no_required)
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


def test_optional_failure_waits_for_required_check(monkeypatch: pytest.MonkeyPatch) -> None:
    """An optional failure cannot terminate the watch while a required check
    is still running (#3054)."""
    initial = _view(9104, "OPEN", "BLOCKED")
    failing = _view(9104, "OPEN", "BLOCKED")
    failing["statusCheckRollup"] = [
        {"name": "regression", "status": "COMPLETED", "conclusion": "FAILURE"},
        {"name": "flake-check", "status": "IN_PROGRESS", "conclusion": None},
    ]
    terminal = _view(9104, "MERGED", "CLEAN")

    result, fake, notified = _watch(
        monkeypatch,
        9104,
        [initial, failing, terminal],
        required=[[_check("flake-check", "IN_PROGRESS", "pending")]],
    )

    assert fake.view_calls == 3
    assert result["state"] == "MERGED"
    assert len(notified) == 1


def test_required_failure_ignores_optional_pending_check(monkeypatch: pytest.MonkeyPatch) -> None:
    """An optional pending job cannot delay a terminal required failure."""
    initial = _view(9105, "OPEN", "BLOCKED")
    failing = _view(9105, "OPEN", "BLOCKED")
    failing["statusCheckRollup"] = [
        {"name": "flake-check", "status": "COMPLETED", "conclusion": "FAILURE"},
        {"name": "regression", "status": "IN_PROGRESS", "conclusion": None},
    ]

    result, fake, notified = _watch(
        monkeypatch,
        9105,
        [initial, failing],
        required=[[_check("flake-check", "FAILURE", "fail")]],
    )

    assert fake.view_calls == 2
    assert result["state"] == "failed"
    assert [failure["name"] for failure in result["failures"]] == ["flake-check"]
    assert len(notified) == 1


def test_startup_failure_is_terminal() -> None:
    """GitHub exposes STARTUP_FAILURE as a completed failure conclusion."""
    check = _check("flake-check", "STARTUP_FAILURE", "pending")

    assert runtime._pr_check_is_terminal(check)
    assert runtime._pr_check_failed(check)


def test_optional_failure_with_no_required_checks_keeps_watching(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """gh reports an empty required set as stderr, not JSON (#3059)."""
    initial = _view(9106, "OPEN", "BLOCKED")
    failing = _view(9106, "OPEN", "BLOCKED")
    failing["statusCheckRollup"] = [
        {"name": "optional", "status": "COMPLETED", "conclusion": "FAILURE"}
    ]
    merged = _view(9106, "MERGED", "CLEAN")

    result, fake, _notified = _watch(
        monkeypatch,
        9106,
        [initial, failing, merged],
        no_required=True,
    )

    assert fake.view_calls == 3
    assert result["state"] == "MERGED"


def test_failure_with_running_check_times_out_explicitly(monkeypatch: pytest.MonkeyPatch) -> None:
    """The watch deadline remains active while failures wait for pending checks,
    and its result preserves both sides of the incomplete aggregate."""
    view = _view(9107, "OPEN", "BLOCKED")
    view["statusCheckRollup"] = [
        {"name": "regression", "status": "COMPLETED", "conclusion": "FAILURE"},
        {"name": "flake-check", "status": "IN_PROGRESS", "conclusion": None},
    ]
    required = [
        _check("regression", "FAILURE", "fail"),
        _check("flake-check", "IN_PROGRESS", "pending"),
    ]
    fake = _ScriptedNu([view], [required])
    monkeypatch.setitem(sys.modules, "nu", fake)

    async def fake_notify(content: str, **meta: object) -> None:
        return None

    monkeypatch.setattr(runtime, "notify", fake_notify)
    result = asyncio.run(
        runtime.watch_pr(9107, auto_merge=False, interval=0.01, timeout=0.02)
    )

    assert result["state"] == "timed out"
    assert [failure["name"] for failure in result["failures"]] == ["regression"]
    assert [pending["name"] for pending in result["pending"]] == ["flake-check"]
