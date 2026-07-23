"""Score rollouts and render the result as a table and a JSON report.

Pure functions (no I/O, no network) so they are unit-tested offline as the CI
gate. The JSON report is the durable artifact committed under ``eval-results/``;
the table is the at-a-glance human view.

Scoring:

- per-behavior pass rate: over every (rollout) of a task that EXPECTS the
  behavior, the fraction in which the judge found it present.
- overall: the mean over all expected (rollout, behavior) pairs.
- longest streak: per task, the longest run of consecutive rollouts in which
  every expected behavior was present. This is the "N agents in a row" signal.
"""

from __future__ import annotations

from collections.abc import Sequence

from .model import Behavior, RolloutResult, TaskCase


def _expects_by_case(tasks: Sequence[TaskCase]) -> dict[str, tuple[str, ...]]:
    return {t.id: t.expects for t in tasks}


def per_behavior_rates(
    results: Sequence[RolloutResult], tasks: Sequence[TaskCase]
) -> dict[str, float]:
    """For each behavior, the fraction of expecting rollouts where it was present."""
    expects = _expects_by_case(tasks)
    seen: dict[str, int] = {}
    hit: dict[str, int] = {}
    for r in results:
        for bid in expects.get(r.case_id, ()):  # only behaviors this task expects
            seen[bid] = seen.get(bid, 0) + 1
            present = (
                r.error is None
                and bid in r.verdicts
                and r.verdicts[bid].present
            )
            if present:
                hit[bid] = hit.get(bid, 0) + 1
    return {bid: hit.get(bid, 0) / n for bid, n in seen.items() if n}


def overall_rate(
    results: Sequence[RolloutResult], tasks: Sequence[TaskCase]
) -> float:
    """Mean presence over all expected (rollout, behavior) pairs."""
    expects = _expects_by_case(tasks)
    total = 0
    present = 0
    for r in results:
        for bid in expects.get(r.case_id, ()):
            total += 1
            if r.error is None and bid in r.verdicts and r.verdicts[bid].present:
                present += 1
    return present / total if total else 0.0


def longest_streak_for(
    results: Sequence[RolloutResult], task: TaskCase
) -> int:
    """Longest run of consecutive rollouts of ``task`` with all behaviors present."""
    rollouts = sorted(
        (r for r in results if r.case_id == task.id), key=lambda r: r.rollout
    )
    best = 0
    run = 0
    for r in rollouts:
        if r.all_expected_present(task.expects):
            run += 1
            best = max(best, run)
        else:
            run = 0
    return best


def max_streak(
    results: Sequence[RolloutResult], tasks: Sequence[TaskCase]
) -> int:
    """The longest all-behaviors-pass streak across all tasks."""
    return max((longest_streak_for(results, t) for t in tasks), default=0)


def summarize(
    results: Sequence[RolloutResult],
    tasks: Sequence[TaskCase],
    behaviors: Sequence[Behavior],
) -> dict[str, object]:
    """Headline numbers for the report and the table."""
    errored = sum(1 for r in results if r.error is not None)
    return {
        "overall": overall_rate(results, tasks),
        "per_behavior": per_behavior_rates(results, tasks),
        "longest_streak": max_streak(results, tasks),
        "per_task_streak": {t.id: longest_streak_for(results, t) for t in tasks},
        "rollouts": len(results),
        "errored": errored,
        "tasks": len(tasks),
        "behaviors": len(behaviors),
        "behavior_defs": [
            {"id": b.id, "name": b.name, "rubric": b.rubric} for b in behaviors
        ],
        "cost": cost_summary(results),
    }


def cases_json(results: Sequence[RolloutResult]) -> list[dict[str, object]]:
    """Per-rollout case rows for the JSON report (includes the raw transcript)."""
    return [
        {
            "case_id": r.case_id,
            "rollout": r.rollout,
            "error": r.error,
            "present": {bid: v.present for bid, v in sorted(r.verdicts.items())},
            "evidence": {bid: v.evidence for bid, v in sorted(r.verdicts.items())},
            "duration_ms": r.duration_ms,
            "input_tokens": r.input_tokens,
            "output_tokens": r.output_tokens,
            "cost_usd": r.cost_usd,
            "transcript": r.transcript,
            "steps": r.steps,
        }
        for r in results
    ]


def cost_summary(results: Sequence[RolloutResult]) -> dict[str, float]:
    """Aggregate time/token/cost metrics over the rollouts."""
    n = len(results) or 1
    return {
        "mean_duration_s": sum(r.duration_ms for r in results) / 1000.0 / n,
        "total_input_tokens": float(sum(r.input_tokens for r in results)),
        "total_output_tokens": float(sum(r.output_tokens for r in results)),
        "total_cost_usd": sum(r.cost_usd for r in results),
    }


def report(
    results: Sequence[RolloutResult],
    tasks: Sequence[TaskCase],
    behaviors: Sequence[Behavior],
    metadata: dict[str, object],
) -> dict[str, object]:
    return {
        "metadata": metadata,
        "summary": summarize(results, tasks, behaviors),
        "cases": cases_json(results),
    }


def render_table(
    results: Sequence[RolloutResult],
    tasks: Sequence[TaskCase],
    behaviors: Sequence[Behavior],
) -> str:
    """A fixed-width per-behavior table, then the headline summary."""
    rates = per_behavior_rates(results, tasks)
    name_by_id = {b.id: b.name for b in behaviors}
    header = f"{'behavior':<18} {'name':<26} {'pass rate':>10}"
    lines = [header, "-" * len(header)]
    for b in behaviors:
        if b.id not in rates:
            continue
        lines.append(f"{b.id:<18} {name_by_id.get(b.id, ''):<26} {rates[b.id]:>9.0%}")
    overall = overall_rate(results, tasks)
    streak = max_streak(results, tasks)
    per_task = {t.id: longest_streak_for(results, t) for t in tasks}
    errored = sum(1 for r in results if r.error is not None)
    lines.append("-" * len(header))
    lines.append(f"{'OVERALL':<18} {'':<26} {overall:>9.0%}")
    lines.append(f"longest all-pass streak: {streak} (per task: {per_task})")
    cost = cost_summary(results)
    lines.append(
        f"rollouts: {len(results)} ({errored} errored), "
        f"tasks: {len(tasks)}, behaviors: {len(behaviors)}"
    )
    lines.append(
        f"cost: mean {cost['mean_duration_s']:.0f}s/rollout, "
        f"{int(cost['total_output_tokens'])} out + {int(cost['total_input_tokens'])} in tokens, "
        f"${cost['total_cost_usd']:.2f} total"
    )
    return "\n".join(lines)
