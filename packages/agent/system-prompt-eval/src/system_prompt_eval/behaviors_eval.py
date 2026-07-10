"""Eval: does the house prompt produce the target DEFAULT behaviors?

Fresh ``claude -p`` rollouts on neutral tasks, judged per behavior (reproduce,
first-principles, experiment, tie-to-issue, named subagents, report-to-playbook).
Headline = overall pass rate; also reports the longest all-behaviors-pass streak
(the "N agents in a row" signal). Safe by default; ``--live`` lets it act.
"""

from __future__ import annotations

from pathlib import Path

from . import data
from .agent import Rollout
from .core import EvalContext, EvalReport
from .report import cases_json, max_streak, overall_rate, render_table, summarize
from .runner import run_eval

NAME = "behaviors"


def run(ctx: EvalContext, *, tasks_path: Path | None = None, behaviors_path: Path | None = None) -> EvalReport:
    behaviors = data.load_behaviors(behaviors_path)
    tasks = data.load_tasks(tasks_path)
    data.validate_expects(tasks, behaviors)
    if ctx.limit is not None:
        tasks = tasks[: ctx.limit]

    rollout = Rollout(
        prompt_file=ctx.prompt_file,
        claude_bin=ctx.claude_bin,
        model=ctx.model,
        effort=ctx.effort,
        live=ctx.live,
        timeout_seconds=ctx.timeout,
    )
    results = run_eval(
        tasks,
        behaviors,
        rollout,
        ctx.judge,
        rollouts=ctx.rollouts,
        max_workers=ctx.max_workers,
        progress=ctx.progress,
    )
    return EvalReport(
        name=NAME,
        headline=overall_rate(results, tasks),
        summary=summarize(results, tasks, behaviors),
        table=render_table(results, tasks, behaviors),
        cases=cases_json(results),
        longest_streak=max_streak(results, tasks),
    )
