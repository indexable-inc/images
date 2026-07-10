"""Offline, deterministic checks for the scoring math.

Runnable as a plain script (``python tests/test_scoring.py``) so the Nix build
can exercise it against the installed package with no test runner, no network,
and no API key. These guard the aggregation logic (pass rates, streaks) that
turns judge verdicts into the committed score.
"""

from __future__ import annotations

from system_prompt_eval.data import load_behaviors, load_tasks, validate_expects
from system_prompt_eval.model import Behavior, BehaviorVerdict, RolloutResult, TaskCase
from system_prompt_eval.report import (
    longest_streak_for,
    max_streak,
    overall_rate,
    per_behavior_rates,
)


def _result(case_id: str, rollout: int, present: dict[str, bool]) -> RolloutResult:
    verdicts = {
        bid: BehaviorVerdict(behavior_id=bid, present=p, evidence="")
        for bid, p in present.items()
    }
    return RolloutResult(case_id=case_id, rollout=rollout, transcript="", verdicts=verdicts)


def test_per_behavior_rate_counts_only_expecting_tasks() -> None:
    tasks = [TaskCase(id="t", task="", expects=("a", "b"))]
    results = [
        _result("t", 0, {"a": True, "b": False}),
        _result("t", 1, {"a": True, "b": True}),
    ]
    rates = per_behavior_rates(results, tasks)
    assert rates["a"] == 1.0
    assert rates["b"] == 0.5


def test_overall_rate_is_pair_mean() -> None:
    tasks = [TaskCase(id="t", task="", expects=("a", "b"))]
    results = [_result("t", 0, {"a": True, "b": False})]
    # 1 of 2 expected pairs present.
    assert overall_rate(results, tasks) == 0.5


def test_unexpected_behavior_not_scored() -> None:
    tasks = [TaskCase(id="t", task="", expects=("a",))]
    # 'b' present but not expected: must not affect the rate.
    results = [_result("t", 0, {"a": True, "b": True})]
    rates = per_behavior_rates(results, tasks)
    assert set(rates) == {"a"}
    assert overall_rate(results, tasks) == 1.0


def test_streak_counts_consecutive_all_pass() -> None:
    task = TaskCase(id="t", task="", expects=("a", "b"))
    results = [
        _result("t", 0, {"a": True, "b": True}),
        _result("t", 1, {"a": True, "b": True}),
        _result("t", 2, {"a": True, "b": False}),  # breaks the streak
        _result("t", 3, {"a": True, "b": True}),
    ]
    assert longest_streak_for(results, task) == 2


def test_streak_respects_rollout_order_not_list_order() -> None:
    task = TaskCase(id="t", task="", expects=("a",))
    # Provided out of order; streak must sort by rollout index first.
    results = [
        _result("t", 2, {"a": True}),
        _result("t", 0, {"a": True}),
        _result("t", 1, {"a": False}),
    ]
    assert longest_streak_for(results, task) == 1


def test_error_rollout_breaks_streak_and_scores_absent() -> None:
    task = TaskCase(id="t", task="", expects=("a",))
    err = RolloutResult(case_id="t", rollout=1, transcript="", error="boom")
    results = [
        _result("t", 0, {"a": True}),
        err,
        _result("t", 2, {"a": True}),
    ]
    assert longest_streak_for(results, task) == 1
    assert overall_rate(results, task and [task]) < 1.0


def test_max_streak_across_tasks() -> None:
    tasks = [
        TaskCase(id="t1", task="", expects=("a",)),
        TaskCase(id="t2", task="", expects=("a",)),
    ]
    results = [
        _result("t1", 0, {"a": True}),
        _result("t1", 1, {"a": False}),
        _result("t2", 0, {"a": True}),
        _result("t2", 1, {"a": True}),
    ]
    assert max_streak(results, tasks) == 2


def test_validate_expects_accepts_known_ids() -> None:
    behaviors = [Behavior(id="a", name="A", rubric="")]
    tasks = [TaskCase(id="t", task="", expects=("a",))]
    validate_expects(tasks, behaviors)  # must not raise


def test_validate_expects_rejects_unknown_id() -> None:
    behaviors = [Behavior(id="a", name="A", rubric="")]
    tasks = [TaskCase(id="t", task="", expects=("a", "typo_id"))]
    error: ValueError | None = None
    try:
        validate_expects(tasks, behaviors)
    except ValueError as exc:
        error = exc
    assert error is not None, "expected ValueError for an unknown expects id"
    assert "typo_id" in str(error)


def test_committed_tasks_expect_only_cataloged_behaviors() -> None:
    """Guards the actual datasets: a bad id here would silently score wrong."""
    validate_expects(load_tasks(), load_behaviors())  # must not raise


def _main() -> None:
    tests = [v for name, v in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"ok: {len(tests)} scoring tests passed")


if __name__ == "__main__":
    _main()
