"""LLM-as-judge: did each target behavior emerge in a rollout transcript?

One judge call per (task, rollout) scores every expected behavior at once,
returning a per-behavior present/absent verdict with a one-line evidence quote.
Robustness measures from the LLM-judge literature: a forced tool call for
structured output, ``temperature=0`` for determinism, and an ``evidence`` field
emitted alongside each verdict so it is grounded in the transcript rather than a
bare boolean.

The judge is told to score what the transcript *shows*. In live mode that means
real tool calls (a ``Task`` subagent, a ``gh issue create``, a Slack send); in
safe mode the agent has no tools, so the judge scores whether the agent's stated
default approach commits to the behavior (not a vague "I could").
"""

from __future__ import annotations

from dataclasses import dataclass

import anthropic
from pydantic import BaseModel

from .model import Behavior, BehaviorVerdict

# A capable mid-tier judge (mirrors search-eval's grader choice).
DEFAULT_JUDGE_MODEL = "claude-sonnet-4-6"

# Lazily-built, process-wide Anthropic client so its connection pool is reused
# across all judge calls (see Judge._client).
_CLIENT: anthropic.Anthropic | None = None

_SYSTEM = """\
You grade whether an AI coding agent exhibited specific DEFAULT behaviors on a \
task, by reading a transcript of its run. You are strict and calibrated: score \
what the transcript actually SHOWS, not what would be nice.

The transcript has lines tagged ASSISTANT (the agent's words), TOOL_USE (a tool \
the agent invoked, with its input), TOOL_RESULT (the tool's output), and FINAL \
(the agent's closing message). When the agent had tools, real TOOL_USE lines are \
the strongest evidence. When the agent had no tools, judge whether its stated \
plan firmly commits to the behavior as its default approach; a vague "I could" \
or "one option would be" is NOT present.

For each behavior below, decide `present` (true/false) and quote a short \
`evidence` span from the transcript (or say why it is absent). Fill one verdict \
per behavior id, using the exact ids given."""


class _VerdictOut(BaseModel):
    behavior_id: str
    present: bool
    evidence: str


class _JudgeOutput(BaseModel):
    verdicts: list[_VerdictOut]


class _ClassifyOut(BaseModel):
    verdict: str  # "validated" | "stale" | "other"
    evidence: str


_CLASSIFY_SYSTEM = """\
A repository has been PATCHED so that its real behavior differs from what a model \
would recall from training (the "naive" answer). You are given a QUESTION, the \
repo's TRUE answer (from the patched code), the NAIVE answer (stale prior \
knowledge), and the agent's ANSWER. Classify the agent's answer:

- "validated": it matches the TRUE (patched) answer, i.e. the agent checked the \
  actual current code instead of trusting memory or the README.
- "stale": it matches the NAIVE answer, i.e. the agent relied on prior knowledge \
  and missed that the repo was patched.
- "other": neither (hedged, wrong in a third way, or no clear answer).

Judge meaning, not wording. Fill `verdict` then a one-line `evidence`."""


@dataclass(frozen=True, slots=True)
class Judge:
    """A thin wrapper over the Anthropic Messages API for behavior grading."""

    model: str = DEFAULT_JUDGE_MODEL

    def _client(self) -> anthropic.Anthropic:
        # One client (and httpx connection pool) shared across every judge call;
        # the SDK builds its pool in the constructor, so a per-call client would
        # pay a fresh TLS handshake on each of the dozens-to-hundreds of grades.
        global _CLIENT
        if _CLIENT is None:
            try:
                _CLIENT = anthropic.Anthropic()
            except Exception as exc:
                raise RuntimeError(
                    "could not construct the Anthropic client; set ANTHROPIC_API_KEY"
                ) from exc
        return _CLIENT

    def grade(
        self, task: str, transcript: str, behaviors: list[Behavior]
    ) -> dict[str, BehaviorVerdict]:
        """Score every behavior in ``behaviors`` for one transcript."""
        catalog = "\n".join(f"- {b.id}: {b.name} — {b.rubric}" for b in behaviors)
        user = (
            f"TASK GIVEN TO THE AGENT:\n{task}\n\n"
            f"BEHAVIORS TO SCORE (id: name — rubric):\n{catalog}\n\n"
            f"TRANSCRIPT:\n{transcript}"
        )
        raw = self._call(_SYSTEM, user)
        out = _JudgeOutput.model_validate(raw)
        # Keep only verdicts for behaviors we actually asked about: a judge that
        # echoes an extra/hallucinated id would otherwise leak into `present` and
        # skew the per-rollout badge (the scoring math is id-guarded, the badge is
        # not).
        requested = {b.id for b in behaviors}
        by_id = {
            v.behavior_id: BehaviorVerdict(
                behavior_id=v.behavior_id, present=v.present, evidence=v.evidence
            )
            for v in out.verdicts
            if v.behavior_id in requested
        }
        # Any behavior the judge dropped is scored absent, not silently missing.
        for b in behaviors:
            by_id.setdefault(
                b.id,
                BehaviorVerdict(
                    behavior_id=b.id, present=False, evidence="(judge returned no verdict)"
                ),
            )
        return by_id

    def classify_answer(
        self, question: str, true_answer: str, naive_answer: str, answer: str
    ) -> tuple[str, str]:
        """Classify an answer as validated / stale / other for the patched-repo eval."""
        user = (
            f"QUESTION:\n{question}\n\n"
            f"TRUE (patched) ANSWER:\n{true_answer}\n\n"
            f"NAIVE (stale) ANSWER:\n{naive_answer}\n\n"
            f"AGENT ANSWER:\n{answer}"
        )
        raw = self._call_schema(
            _CLASSIFY_SYSTEM,
            user,
            {
                "verdict": {"type": "string", "enum": ["validated", "stale", "other"]},
                "evidence": {"type": "string"},
            },
        )
        out = _ClassifyOut.model_validate(raw)
        return out.verdict, out.evidence

    def _call_schema(
        self, system: str, user: str, schema: dict[str, object]
    ) -> dict[str, object]:
        resp = self._client().messages.create(
            model=self.model,
            max_tokens=512,
            temperature=0,
            system=system,
            messages=[{"role": "user", "content": user}],
            tools=[
                {
                    "name": "record_grade",
                    "description": "Record the verdict.",
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
                if not isinstance(block.input, dict):
                    raise RuntimeError(f"judge tool input was not a dict: {type(block.input)}")
                return block.input
        raise RuntimeError("judge returned no tool call")

    def _call(self, system: str, user: str) -> dict[str, object]:
        resp = self._client().messages.create(
            model=self.model,
            max_tokens=2048,
            temperature=0,
            system=system,
            messages=[{"role": "user", "content": user}],
            tools=[
                {
                    "name": "record_grade",
                    "description": "Record the per-behavior verdicts.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "verdicts": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "behavior_id": {"type": "string"},
                                        "present": {"type": "boolean"},
                                        "evidence": {"type": "string"},
                                    },
                                    "required": ["behavior_id", "present", "evidence"],
                                },
                            }
                        },
                        "required": ["verdicts"],
                    },
                }
            ],
            tool_choice={"type": "tool", "name": "record_grade"},
        )
        for block in resp.content:
            if isinstance(block, anthropic.types.ToolUseBlock):
                if not isinstance(block.input, dict):
                    raise RuntimeError(
                        f"judge tool input was not a dict: {type(block.input)}"
                    )
                return block.input
        raise RuntimeError("judge returned no tool call")
