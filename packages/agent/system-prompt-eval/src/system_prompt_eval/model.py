"""Shared data types for the behavioral eval.

Small frozen dataclasses for the values that flow between the runner, the judge,
and the report. Domain data is named, not anonymous tuples.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True, slots=True)
class Behavior:
    """One default behavior the house prompt is supposed to produce.

    ``rubric`` is the judge's instruction: what, concretely, counts as the
    behavior emerging in a transcript.
    """

    id: str
    name: str
    rubric: str


@dataclass(frozen=True, slots=True)
class TaskCase:
    """A neutral task that should organically surface a set of behaviors.

    ``expects`` lists the behavior ids this task is designed to trigger; only
    those are scored for this task (a task that never involves code cannot be
    expected to file a code issue).
    """

    id: str
    task: str
    expects: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class BehaviorVerdict:
    """The judge's yes/no verdict for one behavior in one rollout."""

    behavior_id: str
    present: bool
    evidence: str


@dataclass(slots=True)
class RolloutResult:
    """One ``claude -p`` rollout of one task, with the per-behavior verdicts."""

    case_id: str
    rollout: int
    transcript: str
    verdicts: dict[str, BehaviorVerdict] = field(default_factory=dict)
    error: str | None = None
    # Cost metrics from the rollout's final result event (0 when unavailable).
    duration_ms: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0

    def all_expected_present(self, expected: tuple[str, ...]) -> bool:
        """True iff every expected behavior was judged present (and no error)."""
        if self.error is not None:
            return False
        return all(
            bid in self.verdicts and self.verdicts[bid].present for bid in expected
        )
