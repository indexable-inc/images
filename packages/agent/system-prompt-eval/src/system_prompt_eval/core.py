"""Shared context and result types for the eval registry.

Every eval is a function ``run(ctx: EvalContext) -> EvalReport``. The CLI selects
one or all of them, prints each table, merges the JSON, and applies thresholds
against each report's ``headline`` (a single 0..1 score) so different evals can
share one command.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path

from .judge import Judge

Progress = Callable[[str], None]


def noop(_: str) -> None:
    return None


@dataclass(frozen=True, slots=True)
class EvalContext:
    """Everything an eval needs to run, resolved once by the CLI."""

    prompt_file: Path
    judge: Judge
    # Which agent runs the rollout. "claude" is implemented; "codex" is the seam
    # for the agent x model x effort matrix (deferred, see the codex-backend issue).
    agent_kind: str = "claude"
    claude_bin: str = "claude"
    model: str | None = "opus"
    # Reasoning effort: never "fast"/low for an eval. high is the floor.
    effort: str = "high"
    rollouts: int = 5
    max_workers: int = 4
    live: bool = False
    sandbox: bool = False
    timeout: float = 600.0
    limit: int | None = None
    progress: Progress = noop


@dataclass(slots=True)
class EvalReport:
    """The result of one eval: a headline score, a human table, and JSON cases."""

    name: str
    headline: float
    summary: dict[str, object]
    table: str
    cases: list[dict[str, object]] = field(default_factory=list)
    # Behaviors reports an all-pass streak; other evals leave this None.
    longest_streak: int | None = None

    def to_json(self) -> dict[str, object]:
        return {
            "name": self.name,
            "headline": self.headline,
            "summary": self.summary,
            "longest_streak": self.longest_streak,
            "cases": self.cases,
        }
