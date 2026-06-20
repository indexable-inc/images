"""Orchestrate the eval: run rollouts concurrently, then judge each transcript.

Kept free of presentation and argument parsing so it can be driven from the CLI
or a notebook. Rollouts are independent ``claude -p`` subprocesses, so they run
in a thread pool; judging happens after, one call per (task, rollout).
"""

from __future__ import annotations

from collections.abc import Callable, Sequence
from concurrent.futures import ThreadPoolExecutor

from .agent import AgentError, Rollout
from .judge import Judge
from .model import Behavior, RolloutResult, TaskCase

Progress = Callable[[str], None]


def _noop(_: str) -> None:
    return None


def _jobs(tasks: Sequence[TaskCase], rollouts: int) -> list[tuple[TaskCase, int]]:
    return [(task, i) for task in tasks for i in range(rollouts)]


def run_eval(
    tasks: Sequence[TaskCase],
    behaviors: Sequence[Behavior],
    rollout: Rollout,
    judge: Judge,
    *,
    rollouts: int = 5,
    max_workers: int = 4,
    progress: Progress = _noop,
) -> list[RolloutResult]:
    """Run ``rollouts`` fresh agents per task, capture transcripts, then judge."""
    by_id = {b.id: b for b in behaviors}
    jobs = _jobs(tasks, rollouts)

    def _capture(job: tuple[TaskCase, int]) -> RolloutResult:
        task, idx = job
        progress(f"rollout {task.id}#{idx}")
        try:
            out = rollout.run(task.task)
        except AgentError as exc:
            return RolloutResult(case_id=task.id, rollout=idx, transcript="", error=str(exc))
        return RolloutResult(
            case_id=task.id,
            rollout=idx,
            transcript=out.transcript,
            duration_ms=out.metrics.duration_ms,
            input_tokens=out.metrics.input_tokens,
            output_tokens=out.metrics.output_tokens,
            cost_usd=out.metrics.cost_usd,
            steps=out.steps,
        )

    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        captured = list(pool.map(_capture, jobs))

    expects_by_id = {t.id: t.expects for t in tasks}
    task_by_id = {t.id: t for t in tasks}
    for result in captured:
        if result.error is not None:
            continue
        expected = [by_id[bid] for bid in expects_by_id[result.case_id] if bid in by_id]
        progress(f"judge {result.case_id}#{result.rollout}")
        try:
            result.verdicts = judge.grade(
                task_by_id[result.case_id].task, result.transcript, expected
            )
        except Exception as exc:  # judging failure is recorded, not fatal to the run
            result.error = f"judge failed: {exc}"
    return captured
