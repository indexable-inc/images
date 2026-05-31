"""Shared data types for the eval harness.

Small frozen dataclasses for the values that flow between the backend, the
judge, and the report. Domain data is named, not anonymous tuples.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass(frozen=True, slots=True)
class Hit:
    """One search result, mirroring the `search --json` object shape."""

    path: str
    source: str
    score: float
    start_line: int | None
    num_lines: int | None
    text: str

    @classmethod
    def from_json(cls, obj: dict[str, Any]) -> "Hit":
        return cls(
            path=str(obj["path"]),
            source=str(obj.get("source", "code")),
            score=float(obj.get("score", 0.0)),
            start_line=_opt_int(obj.get("start_line")),
            num_lines=_opt_int(obj.get("num_lines")),
            text=str(obj.get("text", "")),
        )


def _opt_int(value: object) -> int | None:
    if value is None:
        return None
    if isinstance(value, (int, float, str)):
        return int(value)
    raise TypeError(f"expected an int-like value, got {type(value).__name__}")


@dataclass(frozen=True, slots=True)
class RetrievalCase:
    """A Tier A case: a query and its graded gold documents.

    `relevant` maps a corpus-relative path to a relevance grade (``1`` binary,
    or a small integer for graded relevance).
    """

    id: str
    query: str
    relevant: dict[str, float]


@dataclass(frozen=True, slots=True)
class TaskCase:
    """A Tier B case: an agent task whose answer is only in the corpus."""

    id: str
    task: str
    answer: str


@dataclass(frozen=True, slots=True)
class RelevanceGrade:
    """An LLM judge's pointwise relevance verdict for one (query, hit) pair."""

    score: float
    confidence: float
    reasoning: str


@dataclass(frozen=True, slots=True)
class BinaryVerdict:
    """An LLM judge's yes/no verdict with its reasoning (correctness/groundedness)."""

    passed: bool
    reasoning: str


@dataclass(slots=True)
class RetrievalResult:
    """Per-case Tier A outcome: ranking metrics plus the judge's mean relevance."""

    case: RetrievalCase
    retrieved: list[str]
    metrics: dict[str, float]
    judge_relevance: float | None = None
    grades: list[RelevanceGrade] = field(default_factory=list)


@dataclass(slots=True)
class TaskResult:
    """Per-case Tier B outcome: the agent's answer and the correctness verdict."""

    case: TaskCase
    answer: str
    correct: bool
    reasoning: str
    error: str | None = None
