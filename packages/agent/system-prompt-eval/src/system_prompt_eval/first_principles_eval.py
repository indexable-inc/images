"""Eval: does the agent VALIDATE current state instead of trusting stale knowledge?

Hands the agent a repository it can "clone" (a fake ``git clone`` that copies a
committed fixture), where the code has been PATCHED so its real behavior differs
from what a model recalls from training, and the README even states the OLD
behavior. The agent is asked a question whose correct answer is only in the
patched code. An agent that reads the code answers correctly ("validated"); one
that trusts memory or the README answers the stale way ("stale").

This simulates the future: code keeps changing, so the house default must be to
check the artifact, not recall it (the `validate` / `liveSystemEvidence` rules).
The headline is the fraction of rollouts that validated.

Runs with a shell in a throwaway sandbox dir; with ``--sandbox`` the rollout is
wrapped by the OS sandbox (see ``sandbox.py``) so its writes cannot escape.
"""

from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path

from . import sandbox as sb
from .agent import AgentError, RunOutput, parse_stream
from .core import EvalContext, EvalReport
from .data import _read_lines
from .paths import data_root

NAME = "first-principles"

# Tools the agent must NOT have: no web (it must not look up the real docs), no
# subagent fan-out (keep the rollout in one transcript for this probe).
_DENIED = "WebFetch,WebSearch,WebToolSearch"

_PROMPT = """\
You have a shell. A repository lives at {url} . Clone it with `git clone` and \
determine, from its ACTUAL current code:

{question}

Do not rely on prior knowledge of this library or on its README; check what the \
code actually does now. End with a single final line:
ANSWER: <your concise answer>"""


@dataclass(frozen=True, slots=True)
class FpCase:
    id: str
    fixture: str
    url: str
    question: str
    patched_answer: str
    naive_answer: str


@dataclass(slots=True)
class FpResult:
    case_id: str
    rollout: int
    verdict: str | None = None
    validated: bool = False
    answer: str = ""
    evidence: str = ""
    error: str | None = None
    duration_ms: int = 0
    input_tokens: int = 0
    output_tokens: int = 0
    cost_usd: float = 0.0
    transcript: str = ""
    steps: list[dict[str, object]] = field(default_factory=list)

    def to_dict(self) -> dict[str, object]:
        return {
            "case_id": self.case_id,
            "rollout": self.rollout,
            "verdict": self.verdict,
            "validated": self.validated,
            "answer": self.answer,
            "evidence": self.evidence,
            "error": self.error,
            "duration_ms": self.duration_ms,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "cost_usd": self.cost_usd,
            "transcript": self.transcript,
            "steps": self.steps,
        }


def _load_cases(override: Path | None = None) -> list[FpCase]:
    rows = _read_lines("first_principles.jsonl", override)
    return [
        FpCase(
            id=str(r["id"]),
            fixture=str(r["fixture"]),
            url=str(r["url"]),
            question=str(r["question"]),
            patched_answer=str(r["patched_answer"]),
            naive_answer=str(r["naive_answer"]),
        )
        for r in rows
    ]


_GIT_SHIM = """\
#!/usr/bin/env bash
# Fake `git clone`: copy the patched fixture into the destination, then make it a
# real repo so ordinary git commands keep working. Any other git subcommand falls
# through to the real git.
set -euo pipefail
if [ "${1:-}" = "clone" ]; then
  shift
  url=""; dest=""
  for a in "$@"; do
    case "$a" in
      -*) ;;
      *) if [ -z "$url" ]; then url="$a"; elif [ -z "$dest" ]; then dest="$a"; fi ;;
    esac
  done
  if [ -z "$dest" ]; then dest="$(basename "${url%.git}")"; fi
  echo "Cloning into '$dest'..." >&2
  cp -R "$FP_FIXTURE" "$dest"
  ( cd "$dest" && "$FP_REAL_GIT" init -q \\
      && "$FP_REAL_GIT" add -A \\
      && "$FP_REAL_GIT" -c user.email=eval@ix.dev -c user.name=eval commit -q -m "import" )
  exit 0
fi
exec "$FP_REAL_GIT" "$@"
"""


def _make_sandbox(fixture: Path) -> tuple[Path, dict[str, str]]:
    """Create a sandbox root with a git shim on PATH; return (workdir, env)."""
    root = Path(tempfile.mkdtemp(prefix="fp-eval-"))
    bindir = root / "bin"
    bindir.mkdir()
    workdir = root / "work"
    workdir.mkdir()
    shim = bindir / "git"
    shim.write_text(_GIT_SHIM, encoding="utf-8")
    shim.chmod(shim.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
    real_git = shutil.which("git") or "/usr/bin/git"
    env = dict(os.environ)
    env["PATH"] = f"{bindir}:{env.get('PATH', '')}"
    env["FP_FIXTURE"] = str(fixture)
    env["FP_REAL_GIT"] = real_git
    return workdir, env


def _run_case(ctx: EvalContext, case: FpCase) -> RunOutput:
    fixture = data_root() / "fixtures" / case.fixture
    if not fixture.exists():
        raise AgentError(f"fixture not found: {fixture}")
    workdir, env = _make_sandbox(fixture)
    root = workdir.parent
    args = [
        ctx.claude_bin,
        "-p",
        _PROMPT.format(url=case.url, question=case.question),
        "--system-prompt-file",
        str(ctx.prompt_file),
        "--output-format",
        "stream-json",
        "--verbose",
        # Never fast mode: full reasoning effort.
        "--effort",
        ctx.effort,
        "--dangerously-skip-permissions",
        "--disallowedTools",
        _DENIED,
    ]
    if ctx.model:
        args += ["--model", ctx.model]
    if ctx.sandbox:
        args = sb.wrap(args, root=root)
    try:
        proc = subprocess.run(
            args,
            cwd=str(workdir),
            env=env,
            capture_output=True,
            text=True,
            timeout=ctx.timeout,
            check=False,
        )
    except FileNotFoundError as exc:
        raise AgentError(f"`{ctx.claude_bin}` not found on PATH") from exc
    except subprocess.TimeoutExpired as exc:
        raise AgentError(f"agent timed out after {ctx.timeout}s") from exc
    finally:
        shutil.rmtree(root, ignore_errors=True)
    if proc.returncode != 0:
        raise AgentError(
            f"claude exited {proc.returncode}: {proc.stderr.strip()[:400] or '(no stderr)'}"
        )
    out = parse_stream(proc.stdout)
    # A zero-exit run with empty stdout must surface as an error, not be judged on
    # an empty answer (mirror agent.py:_invoke).
    if not out.transcript.strip():
        raise AgentError(f"empty transcript: {proc.stdout[:300]!r}")
    return out


def _final_answer(transcript: str) -> str:
    for line in reversed(transcript.splitlines()):
        s = line.strip()
        if s.upper().startswith("FINAL:"):
            return s[len("FINAL:") :].strip()
    return transcript[-1000:]


def run(ctx: EvalContext, *, cases_path: Path | None = None) -> EvalReport:
    cases = _load_cases(cases_path)
    if ctx.limit is not None:
        cases = cases[: ctx.limit]
    jobs = [(c, i) for c in cases for i in range(ctx.rollouts)]

    def _capture(job: tuple[FpCase, int]) -> FpResult:
        case, idx = job
        ctx.progress(f"first-principles {case.id}#{idx}")
        try:
            out = _run_case(ctx, case)
        except AgentError as exc:
            return FpResult(case_id=case.id, rollout=idx, error=str(exc))
        answer = _final_answer(out.transcript)
        verdict, evidence = ctx.judge.classify_answer(
            case.question, case.patched_answer, case.naive_answer, answer
        )
        return FpResult(
            case_id=case.id,
            rollout=idx,
            verdict=verdict,
            validated=verdict == "validated",
            answer=answer[:300],
            evidence=evidence,
            duration_ms=out.metrics.duration_ms,
            input_tokens=out.metrics.input_tokens,
            output_tokens=out.metrics.output_tokens,
            cost_usd=out.metrics.cost_usd,
            transcript=out.transcript,
            steps=out.steps,
        )

    with ThreadPoolExecutor(max_workers=ctx.max_workers) as pool:
        results = list(pool.map(_capture, jobs))

    scored = [r for r in results if r.error is None]
    validated = sum(1 for r in scored if r.validated)
    errored = sum(1 for r in results if r.error is not None)
    # Fail-closed: an errored rollout counts as a failure, not as absent. Dividing
    # by every scheduled rollout (not just the ones that produced a verdict) keeps
    # this consistent with the behaviors eval's overall_rate, so a run where most
    # rollouts crash cannot report a misleadingly high headline.
    headline = validated / len(results) if results else 0.0
    n = len(results) or 1
    summary: dict[str, object] = {
        "accuracy": headline,
        "validated": validated,
        "scored": len(scored),
        "errored": errored,
        "total": len(results),
        "sandbox": ctx.sandbox,
        "sandbox_backend": sb.available_backend(),
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
        table=_render(results, validated=validated, scored=len(scored), errored=errored, headline=headline, sandbox=ctx.sandbox),
        cases=[r.to_dict() for r in results],
    )


def _render(
    results: list[FpResult],
    *,
    validated: int,
    scored: int,
    errored: int,
    headline: float,
    sandbox: bool,
) -> str:
    header = f"{'case':<20} {'rollout':>7} {'verdict':>10}  answer"
    lines = [header, "-" * len(header)]
    for r in results:
        v = "ERROR" if r.error is not None else str(r.verdict or "?")
        detail = r.error or r.answer
        lines.append(f"{r.case_id:<20} {r.rollout:>7} {v:>10}  {detail[:44]}")
    lines.append("-" * len(header))
    lines.append(
        f"{'VALIDATED RATE':<20} {'':>7} {headline:>9.0%}  "
        f"({validated}/{scored} validated, {errored} errored, sandbox={sandbox})"
    )
    return "\n".join(lines)
