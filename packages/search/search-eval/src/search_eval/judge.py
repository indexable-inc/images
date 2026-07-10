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

import anthropic
from pydantic import BaseModel

from .model import BinaryVerdict, Hit, RelevanceGrade


class _RelevanceOutput(BaseModel):
    """Structured output from the judge's relevance tool call."""

    reasoning: str
    score: float
    confidence: float


class _CorrectnessOutput(BaseModel):
    """Structured output from the judge's correctness tool call."""

    reasoning: str
    passed: bool


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


def _grade_tool(schema: dict[str, object]) -> anthropic.types.ToolParam:
    tool: anthropic.types.ToolParam = {
        "name": "record_grade",
        "description": "Record the grading verdict.",
        "input_schema": {
            "type": "object",
            "properties": schema,
            "required": list(schema),
        },
    }
    return tool


def _tool_input(response: anthropic.types.Message) -> dict[str, object]:
    for block in response.content:
        if isinstance(block, anthropic.types.ToolUseBlock):
            if not isinstance(block.input, dict):
                raise RuntimeError(f"judge tool input was not a dict: {type(block.input)}")
            return block.input
    raise RuntimeError("judge returned no tool call")


@dataclass(frozen=True, slots=True)
class Judge:
    """A thin wrapper over the Anthropic Messages API for grading."""

    model: str = DEFAULT_JUDGE_MODEL

    def _client(self) -> anthropic.Anthropic:
        try:
            return anthropic.Anthropic()
        except Exception as exc:
            raise RuntimeError(
                "could not construct the Anthropic client; set ANTHROPIC_API_KEY"
            ) from exc

    def _call(
        self, system: str, user: str, schema: dict[str, object]
    ) -> dict[str, object]:
        resp = self._client().messages.create(
            model=self.model,
            max_tokens=512,
            temperature=0,
            system=system,
            messages=[{"role": "user", "content": user}],
            tools=[_grade_tool(schema)],
            tool_choice={"type": "tool", "name": "record_grade"},
        )
        return _tool_input(resp)

    def grade_relevance(self, query: str, hit: Hit) -> RelevanceGrade:
        """Pointwise relevance of one hit to the query (Tier A)."""
        user = f"QUERY:\n{query}\n\nRESULT (path: {hit.path}):\n{hit.text[:2000]}"
        raw = self._call(
            _RELEVANCE_RUBRIC,
            user,
            {
                "reasoning": {"type": "string"},
                "score": {"type": "number", "minimum": 0, "maximum": 1},
                "confidence": {"type": "number", "minimum": 0, "maximum": 1},
            },
        )
        out = _RelevanceOutput.model_validate(raw)
        return RelevanceGrade(
            score=_clamp(out.score),
            confidence=_clamp(out.confidence),
            reasoning=out.reasoning,
        )

    def grade_correctness(self, question: str, gold: str, answer: str) -> BinaryVerdict:
        """Whether ``answer`` conveys the gold fact (Tier B)."""
        user = f"QUESTION:\n{question}\n\nGOLD ANSWER:\n{gold}\n\nCANDIDATE ANSWER:\n{answer}"
        raw = self._call(
            _CORRECTNESS_RUBRIC,
            user,
            {"reasoning": {"type": "string"}, "passed": {"type": "boolean"}},
        )
        out = _CorrectnessOutput.model_validate(raw)
        return BinaryVerdict(
            passed=out.passed,
            reasoning=out.reasoning,
        )


def _clamp(value: float) -> float:
    return max(0.0, min(1.0, value))
