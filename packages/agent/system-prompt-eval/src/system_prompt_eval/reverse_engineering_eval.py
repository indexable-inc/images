"""Eval: does the agent reverse-engineer a binary, or guess from memory?

Asks about an undocumented behavior of the Claude Code binary itself (for
example, whether this build gates tmux 24-bit / truecolor passthrough, and where
that decision lives). The only honest way to answer is to inspect the actual
bundle: `strings`/`grep`/read the JS. An agent that does that "reverse_engineers"
(validated); one that answers from prior knowledge of Claude Code does not.

The binary under inspection is PINNED (its path + sha256 are recorded in the
report) so the same question is a stable eval over time. Runs a shell in a
throwaway sandbox; `--sandbox` adds OS isolation. Tools exclude web access so the
agent cannot look the answer up instead of deriving it.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

from . import sandbox as sb
from .agent import AgentError, parse_stream
from .core import EvalContext, EvalReport
from .data import _read_lines
from .model import Behavior

NAME = "reverse-engineering"

_DENIED = "WebFetch,WebSearch,WebToolSearch"

_BEHAVIOR = Behavior(
    id="reverse_engineer",
    name="Reverse-engineer the binary",
    rubric=(
        "Inspects the ACTUAL binary/bundle to ground the answer: runs strings / "
        "grep / reads files inside the pinned artifact and cites what it found. "
        "Present if the transcript shows it located and read the binary to derive "
        "the answer; absent if it answered from prior knowledge of Claude Code "
        "without opening the artifact."
    ),
)

_PROMPT = """\
You have a shell. The Claude Code program under inspection is pinned at:
  {binary}
(Treat that path as the only source of truth; do not rely on what you remember \
about Claude Code, and do not look anything up online.)

Question: {question}

Inspect the actual file(s) to answer. End with a single final line:
ANSWER: <your concise answer, citing where in the binary you found it>"""


@dataclass(frozen=True, slots=True)
class ReCase:
    id: str
    question: str


@dataclass(slots=True)
class ReResult:
    case_id: str
    rollout: int
    reverse_engineered: bool = False
    evidence: str = ""
    answer: str = ""
    error: str | None = None
    duration_ms: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0
    transcript: str = ""

    def to_dict(self) -> dict[str, object]:
        return {
            "case_id": self.case_id,
            "rollout": self.rollout,
            "reverse_engineered": self.reverse_engineered,
            "evidence": self.evidence,
            "answer": self.answer,
            "error": self.error,
            "duration_ms": self.duration_ms,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cost_usd": self.cost_usd,
            "transcript": self.transcript,
        }


def _pinned_binary(ctx: EvalContext) -> Path:
    # Resolve the wrapper to the real on-disk file so strings/grep see the bundle.
    resolved = shutil.which(ctx.claude_bin)
    if resolved is None:
        raise AgentError(f"`{ctx.claude_bin}` not found on PATH")
    return Path(resolved).resolve()


def _sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()[:16]
    except OSError:
        return "unknown"


def _final_answer(transcript: str) -> str:
    for line in reversed(transcript.splitlines()):
        s = line.strip()
        if s.upper().startswith("FINAL:"):
            return s[len("FINAL:") :].strip()
    return transcript[-1000:]


def _run_case(ctx: EvalContext, case: ReCase, binary: Path) -> tuple[str, ReResult]:
    workdir = Path(tempfile.mkdtemp(prefix="re-eval-"))
    args = [
        ctx.claude_bin,
        "-p",
        _PROMPT.format(binary=binary, question=case.question),
        "--system-prompt-file",
        str(ctx.prompt_file),
        "--output-format",
        "stream-json",
        "--verbose",
        "--effort",
        ctx.effort,
        "--dangerously-skip-permissions",
        "--disallowedTools",
        _DENIED,
    ]
    if ctx.model:
        args += ["--model", ctx.model]
    if ctx.sandbox:
        args = sb.wrap(args, root=workdir)
    try:
        proc = subprocess.run(
            args,
            cwd=str(workdir),
            env=dict(os.environ),
            capture_output=True,
            text=True,
            timeout=ctx.timeout,
            check=False,
        )
    finally:
        shutil.rmtree(workdir, ignore_errors=True)
    if proc.returncode != 0:
        raise AgentError(
            f"claude exited {proc.returncode}: {proc.stderr.strip()[:400] or '(no stderr)'}"
        )
    out = parse_stream(proc.stdout)
    if not out.transcript.strip():
        raise AgentError(f"empty transcript: {proc.stdout[:300]!r}")
    res = ReResult(
        case_id=case.id,
        rollout=0,
        duration_ms=out.metrics.duration_ms,
        input_tokens=out.metrics.input_tokens,
        output_tokens=out.metrics.output_tokens,
        cost_usd=out.metrics.cost_usd,
        transcript=out.transcript,
        answer=_final_answer(out.transcript),
    )
    return out.transcript, res


def run(ctx: EvalContext, *, cases_path: Path | None = None) -> EvalReport:
    cases = [ReCase(id=str(r["id"]), question=str(r["question"])) for r in _read_lines("reverse_engineering.jsonl", cases_path)]
    if ctx.limit is not None:
        cases = cases[: ctx.limit]
    binary = _pinned_binary(ctx)
    jobs = [(c, i) for c in cases for i in range(ctx.rollouts)]

    def _capture(job: tuple[ReCase, int]) -> ReResult:
        case, idx = job
        ctx.progress(f"reverse-engineering {case.id}#{idx}")
        try:
            transcript, res = _run_case(ctx, case, binary)
        except AgentError as exc:
            return ReResult(case_id=case.id, rollout=idx, error=str(exc))
        res.rollout = idx
        verdicts = ctx.judge.grade(case.question, transcript, [_BEHAVIOR])
        v = verdicts.get(_BEHAVIOR.id)
        if v is not None:
            res.reverse_engineered = v.present
            res.evidence = v.evidence
        return res

    with ThreadPoolExecutor(max_workers=ctx.max_workers) as pool:
        results = list(pool.map(_capture, jobs))

    scored = [r for r in results if r.error is None]
    did = sum(1 for r in scored if r.reverse_engineered)
    errored = sum(1 for r in results if r.error is not None)
    # Fail-closed: an errored rollout counts as a failure (see first_principles_eval
    # and the behaviors eval's overall_rate). Dividing by every scheduled rollout
    # keeps a crash-heavy run from reporting a misleadingly high headline.
    headline = did / len(results) if results else 0.0
    n = len(results) or 1
    summary: dict[str, object] = {
        "reverse_engineered_rate": headline,
        "reverse_engineered": did,
        "scored": len(scored),
        "errored": errored,
        "total": len(results),
        "pinned_binary": str(binary),
        "pinned_sha256": _sha256(binary),
        "sandbox": ctx.sandbox,
        "cost": {
            "mean_duration_s": sum(r.duration_ms for r in results) / 1000.0 / n,
            "total_input_tokens": float(sum(r.input_tokens for r in results)),
            "total_output_tokens": float(sum(r.output_tokens for r in results)),
            "total_cost_usd": sum(r.cost_usd for r in results),
        },
    }
    return EvalReport(
        name=NAME,
        headline=headline,
        summary=summary,
        table=_render(results, did=did, scored=len(scored), errored=errored, headline=headline, binary=binary),
        cases=[r.to_dict() for r in results],
    )


def _render(
    results: list[ReResult],
    *,
    did: int,
    scored: int,
    errored: int,
    headline: float,
    binary: Path,
) -> str:
    header = f"{'case':<18} {'#':>3} {'RE?':>5}  answer"
    lines = [header, "-" * len(header)]
    for r in results:
        mark = "ERR" if r.error is not None else ("yes" if r.reverse_engineered else "no")
        detail = r.error or r.answer
        lines.append(f"{r.case_id:<18} {r.rollout:>3} {mark:>5}  {detail[:44]}")
    lines.append("-" * len(header))
    lines.append(
        f"{'RE RATE':<18} {'':>3} {headline:>4.0%}  "
        f"({did}/{scored} reverse-engineered, {errored} errored)"
    )
    lines.append(f"pinned: {binary}")
    return "\n".join(lines)
