"""Drive a fresh ``claude -p`` rollout under the house prompt and capture it.

The prompt is loaded the same way a production session loads it
(``--system-prompt-file``), so the rollout exercises the real default behaviors.

Two modes:

- **safe** (default): ``--allowedTools ""`` denies every tool, so the agent
  cannot create a real GitHub issue, post to Slack, or open a playbook PR. It can
  only describe the approach it would take, which is enough to judge whether the
  prompt makes the behaviors the agent's default plan. Cheap and side-effect-free,
  so the committed eval can be rerun forever.
- **live** (``--live``): ``--dangerously-skip-permissions`` lets the rollout
  actually act (spawn named subagents, file the issue, post the link), and the
  judge scores from the real tool calls. This is the full-send validation, and it
  produces real artifacts.

We capture ``--output-format stream-json`` (not the plain ``json`` envelope) so
the transcript includes every assistant turn and tool call, which is what the
judge reads to decide whether a behavior actually happened versus was merely
claimed.
"""

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

# Compact a tool input/result to keep the JUDGE transcript inside its budget
# while still showing the load-bearing tokens (a `gh issue create`, a Task
# subagent description, a slack send). The full, untruncated detail is kept
# separately in `steps` for the HTML scorecard to render.
_BLOCK_CAP = 600
_TRANSCRIPT_CAP = 18000
# Generous per-step cap for the rich HTML timeline: effectively full, but a guard
# against a pathological multi-MB tool result blowing up the page.
_STEP_CAP = 40000


class AgentError(RuntimeError):
    """The agent process failed to produce a usable transcript."""


@dataclass(frozen=True, slots=True)
class RunMetrics:
    """Cost metrics for one rollout, read from the stream's final result event."""

    duration_ms: int = 0
    num_turns: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0

    def to_dict(self) -> dict[str, object]:
        return {
            "duration_ms": self.duration_ms,
            "num_turns": self.num_turns,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cost_usd": self.cost_usd,
        }


@dataclass(frozen=True, slots=True)
class RunOutput:
    """One rollout's compact judge transcript, full step timeline, and metrics.

    ``transcript`` is the compacted text the LLM judge reads. ``steps`` is the
    full, ordered action timeline (assistant prose, thinking, every tool call with
    its input, every tool result, the final answer) for the HTML scorecard.
    """

    transcript: str
    metrics: RunMetrics
    steps: list[dict[str, object]]


@dataclass(frozen=True, slots=True)
class Rollout:
    """One headless rollout configuration."""

    prompt_file: Path
    claude_bin: str = "claude"
    model: str | None = "opus"
    effort: str = "high"
    live: bool = False
    timeout_seconds: float = 600.0

    def run(self, task: str) -> RunOutput:
        """Run ``claude -p`` on ``task`` and return its transcript + metrics."""
        args = [
            self.claude_bin,
            "-p",
            task,
            "--system-prompt-file",
            str(self.prompt_file),
            "--output-format",
            "stream-json",
            "--verbose",
            # Never fast mode: evals run at full reasoning effort.
            "--effort",
            self.effort,
        ]
        if self.model:
            args += ["--model", self.model]
        if self.live:
            # Real actions: the rollout may file issues, post to Slack, open PRs.
            args += ["--dangerously-skip-permissions"]
        else:
            # No tools: the agent can only narrate its default approach.
            args += ["--allowedTools", ""]
        return self._invoke(args)

    def _invoke(self, args: list[str]) -> RunOutput:
        try:
            proc = subprocess.run(
                args,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                check=False,
                env=dict(os.environ),
            )
        except FileNotFoundError as exc:
            raise AgentError(f"`{self.claude_bin}` not found on PATH") from exc
        except subprocess.TimeoutExpired as exc:
            raise AgentError(f"agent timed out after {self.timeout_seconds}s") from exc
        if proc.returncode != 0:
            raise AgentError(
                f"claude exited {proc.returncode}: "
                f"{proc.stderr.strip()[:400] or '(no stderr)'}"
            )
        out = parse_stream(proc.stdout)
        if not out.transcript.strip():
            raise AgentError(f"empty transcript: {proc.stdout[:300]!r}")
        return out


def parse_stream(stdout: str) -> RunOutput:
    """Flatten stream-json into a judge transcript, a full step timeline, metrics."""
    parts: list[str] = []
    steps: list[dict[str, object]] = []
    metrics = RunMetrics()
    for raw in stdout.splitlines():
        line = raw.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        if kind == "assistant":
            _assistant(event, parts, steps)
        elif kind == "user":
            _tool_results(event, parts, steps)
        elif kind == "result":
            result = str(event.get("result", "")).strip()
            parts.append("FINAL: " + result)
            steps.append({"kind": "final", "text": result[:_STEP_CAP]})
            metrics = _metrics_from_result(event)
    text = "\n".join(p for p in parts if p)
    return RunOutput(transcript=text[:_TRANSCRIPT_CAP], metrics=metrics, steps=steps)


def _metrics_from_result(event: dict[str, object]) -> RunMetrics:
    usage = event.get("usage")
    usage_d = usage if isinstance(usage, dict) else {}
    return RunMetrics(
        duration_ms=_int(event.get("duration_ms")),
        num_turns=_int(event.get("num_turns")),
        input_tokens=_int(usage_d.get("input_tokens")),
        output_tokens=_int(usage_d.get("output_tokens")),
        cost_usd=_float(event.get("total_cost_usd")),
    )


def _int(value: object) -> int:
    return int(value) if isinstance(value, (int, float)) else 0


def _float(value: object) -> float:
    return float(value) if isinstance(value, (int, float)) else 0.0




def _assistant(
    event: dict[str, object], parts: list[str], steps: list[dict[str, object]]
) -> None:
    message = event.get("message")
    if not isinstance(message, dict):
        return
    content = message.get("content")
    if not isinstance(content, list):
        return
    for block in content:
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype == "text":
            text = str(block.get("text", "")).strip()
            parts.append("ASSISTANT: " + text)
            steps.append({"kind": "text", "text": text[:_STEP_CAP]})
        elif btype == "thinking":
            steps.append(
                {"kind": "thinking", "text": str(block.get("thinking", "")).strip()[:_STEP_CAP]}
            )
        elif btype == "tool_use":
            name = str(block.get("name", "?"))
            raw_input = block.get("input", {})
            payload = json.dumps(raw_input, default=str)
            parts.append(f"TOOL_USE {name}: {payload[:_BLOCK_CAP]}")
            steps.append(
                {
                    "kind": "tool_use",
                    "name": name,
                    "input": json.dumps(raw_input, indent=2, default=str)[:_STEP_CAP],
                }
            )


def _tool_results(
    event: dict[str, object], parts: list[str], steps: list[dict[str, object]]
) -> None:
    message = event.get("message")
    if not isinstance(message, dict):
        return
    content = message.get("content")
    if not isinstance(content, list):
        return
    for block in content:
        if not isinstance(block, dict) or block.get("type") != "tool_result":
            continue
        text = _result_text(block.get("content"))
        is_error = bool(block.get("is_error"))
        parts.append("TOOL_RESULT: " + text[:_BLOCK_CAP])
        steps.append({"kind": "tool_result", "text": text[:_STEP_CAP], "is_error": is_error})


def _result_text(content: object) -> str:
    if isinstance(content, str):
        return content.strip()
    if isinstance(content, list):
        chunks = [
            str(b.get("text", ""))
            for b in content
            if isinstance(b, dict) and b.get("type") == "text"
        ]
        return " ".join(chunks).strip()
    return ""
