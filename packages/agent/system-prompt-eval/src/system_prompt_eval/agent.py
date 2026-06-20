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

# Compact a tool input/result to keep the transcript inside the judge's budget
# while still showing the load-bearing tokens (a `gh issue create`, a Task
# subagent description, a slack send).
_BLOCK_CAP = 600
_TRANSCRIPT_CAP = 18000


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
    """One rollout's compacted transcript plus its cost metrics."""

    transcript: str
    metrics: RunMetrics


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
    """Flatten stream-json into a transcript and pull the final result metrics."""
    parts: list[str] = []
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
            parts.extend(_assistant_blocks(event))
        elif kind == "user":
            parts.extend(_tool_results(event))
        elif kind == "result":
            parts.append("FINAL: " + str(event.get("result", "")).strip())
            metrics = _metrics_from_result(event)
    text = "\n".join(p for p in parts if p)
    return RunOutput(transcript=text[:_TRANSCRIPT_CAP], metrics=metrics)


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




def _assistant_blocks(event: dict[str, object]) -> list[str]:
    message = event.get("message")
    if not isinstance(message, dict):
        return []
    content = message.get("content")
    if not isinstance(content, list):
        return []
    out: list[str] = []
    for block in content:
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype == "text":
            out.append("ASSISTANT: " + str(block.get("text", "")).strip())
        elif btype == "tool_use":
            name = str(block.get("name", "?"))
            payload = json.dumps(block.get("input", {}), default=str)[:_BLOCK_CAP]
            out.append(f"TOOL_USE {name}: {payload}")
    return out


def _tool_results(event: dict[str, object]) -> list[str]:
    message = event.get("message")
    if not isinstance(message, dict):
        return []
    content = message.get("content")
    if not isinstance(content, list):
        return []
    out: list[str] = []
    for block in content:
        if not isinstance(block, dict) or block.get("type") != "tool_result":
            continue
        out.append("TOOL_RESULT: " + _result_text(block.get("content"))[:_BLOCK_CAP])
    return out


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
