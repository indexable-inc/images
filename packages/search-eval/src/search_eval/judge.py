"""LLM-as-judge grading, following Exa's open-eval rubric.

Two grading modes, matching how the retrieval community separates concerns:

- **Pointwise relevance** (Tier A): score each ``(query, result)`` pair on a
  0-1 scale with an explicit rubric. This is Exa's "result grading" mode.
- **Correctness** (Tier B): a yes/no verdict on whether an agent's final answer
  matches the gold answer. This is the generative half of Exa's WebCode split.

Robustness measures from the LLM-judge literature: a forced tool call for
structured output, ``temperature=0`` for determinism, and a ``reasoning`` field
emitted before the score so the verdict is chain-of-thought-grounded rather than
a bare number. Position bias is moot here because grading is pointwise (one item
at a time), not pairwise.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, cast

import anthropic

from .model import BinaryVerdict, Hit, RelevanceGrade

# A capable mid-tier judge, the analog of the GPT-4.1 grader Exa reports.
DEFAULT_JUDGE_MODEL = "claude-sonnet-4-6"

_RELEVANCE_RUBRIC = """\
You grade how well a single search RESULT answers a developer's QUERY over a code \
search index. Be strict and calibrated.

Score on a 0.0-1.0 scale:
- 1.0: the result directly and fully answers the query (the right file/section).
- 0.6-0.9: relevant and useful, but partial or adjacent.
- 0.1-0.5: loosely related; shares vocabulary but does not answer the query.
- 0.0: irrelevant.

Fill `reasoning` first with one or two sentences, then `score`, then your \
`confidence` (0.0-1.0) that another careful grader would agree."""

_CORRECTNESS_RUBRIC = """\
You grade whether a candidate ANSWER is factually correct given the GOLD answer \
to a question about a codebase. Judge meaning, not wording: a different phrasing, \
unit, or rounding that conveys the same fact is correct. A missing, hedged, or \
wrong value is incorrect.

Fill `reasoning` first with one sentence, then `passed` (true/false)."""


@dataclass(frozen=True, slots=True)
class Judge:
    """A thin wrapper over the Anthropic Messages API for grading."""

    model: str = DEFAULT_JUDGE_MODEL

    def _client(self) -> anthropic.Anthropic:
        try:
            return anthropic.Anthropic()
        except Exception as exc:  # noqa: BLE001 - surface a clear setup error
            raise RuntimeError(
                "could not construct the Anthropic client; set ANTHROPIC_API_KEY"
            ) from exc

    def _call(
        self, system: str, user: str, schema: dict[str, object]
    ) -> dict[str, Any]:
        resp = self._client().messages.create(
            model=self.model,
            max_tokens=512,
            temperature=0,
            system=system,
            messages=[{"role": "user", "content": user}],
            tools=[
                {
                    "name": "record_grade",
                    "description": "Record the grading verdict.",
                    "input_schema": {
                        "type": "object",
                        "properties": schema,
                        "required": list(schema.keys()),
                    },
                }
            ],
            tool_choice={"type": "tool", "name": "record_grade"},
        )
        for block in resp.content:
            if isinstance(block, anthropic.types.ToolUseBlock):
                out = cast("dict[str, Any]", block.input)
                missing = [key for key in schema if key not in out]
                if missing:
                    # A grade missing required fields is an error, not a default:
                    # silently treating it as 0/false would hide a broken judge.
                    raise RuntimeError(f"judge omitted required fields: {missing}")
                return out
        raise RuntimeError("judge returned no tool call")

    def grade_relevance(self, query: str, hit: Hit) -> RelevanceGrade:
        """Pointwise relevance of one hit to the query (Tier A)."""
        user = f"QUERY:\n{query}\n\nRESULT (path: {hit.path}):\n{hit.text[:2000]}"
        out = self._call(
            _RELEVANCE_RUBRIC,
            user,
            {
                "reasoning": {"type": "string"},
                "score": {"type": "number", "minimum": 0, "maximum": 1},
                "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            },
        )
        return RelevanceGrade(
            score=_clamp(float(out.get("score", 0.0))),
            confidence=_clamp(float(out.get("confidence", 0.0))),
            reasoning=str(out.get("reasoning", "")),
        )

    def grade_correctness(self, question: str, gold: str, answer: str) -> BinaryVerdict:
        """Whether ``answer`` conveys the gold fact (Tier B)."""
        user = f"QUESTION:\n{question}\n\nGOLD ANSWER:\n{gold}\n\nCANDIDATE ANSWER:\n{answer}"
        out = self._call(
            _CORRECTNESS_RUBRIC,
            user,
            {"reasoning": {"type": "string"}, "passed": {"type": "boolean"}},
        )
        return BinaryVerdict(
            passed=bool(out.get("passed", False)),
            reasoning=str(out.get("reasoning", "")),
        )


def _clamp(value: float) -> float:
    return max(0.0, min(1.0, value))
