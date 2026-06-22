"""`system-prompt-eval`: score the house prompt over a registry of evals.

One command, many evals. ``run --eval all`` runs every eval; ``run --eval
<name>`` runs one. Each eval loads the prompt the same way a production session
does and reports a 0..1 headline plus a JSON report committed under
``eval-results/`` to track the score over time.

Evals:
- ``behaviors``: do the target default behaviors emerge (reproduce, first
  principles, experiment, tie-to-issue, named subagents, report-to-playbook)?
  Safe by default; ``--live`` lets rollouts act for real.
- ``first-principles``: given a PATCHED repo via a fake ``git clone``, does the
  agent validate the current code instead of trusting stale knowledge? Runs a
  shell in a throwaway dir; ``--sandbox`` adds OS isolation.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import subprocess
import sys
import tempfile
from pathlib import Path

from . import behaviors_eval, first_principles_eval, reverse_engineering_eval
from .core import EvalContext, EvalReport
from .judge import DEFAULT_JUDGE_MODEL, Judge
from .render import resolve_prompt

# The eval registry: name -> runner. Keep names stable; they are the CLI surface
# and the committed-result keys.
EVALS: dict[str, str] = {
    behaviors_eval.NAME: "do the target default behaviors emerge?",
    first_principles_eval.NAME: "validate a patched repo vs trust stale knowledge?",
    reverse_engineering_eval.NAME: "reverse-engineer a pinned binary vs guess from memory?",
}


def _progress(message: str) -> None:
    print(f"  … {message}", file=sys.stderr, flush=True)


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="system-prompt-eval", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list", help="list the available evals")

    run = sub.add_parser("run", help="run one or all evals and score them")
    run.add_argument(
        "--eval",
        default="all",
        choices=("all", *EVALS.keys()),
        help="which eval to run (default: all)",
    )
    run.add_argument("--rollouts", type=int, default=5, help="rollouts per task/case")
    run.add_argument("--max-workers", type=int, default=4, help="concurrent agents")
    run.add_argument("--limit", type=int, default=None, help="only the first N tasks/cases")
    run.add_argument(
        "--live",
        action="store_true",
        help="behaviors: let rollouts act for real (files issues, posts to Slack)",
    )
    run.add_argument(
        "--sandbox",
        action="store_true",
        help="first-principles: wrap rollouts in the OS sandbox (sandbox-exec/bwrap)",
    )
    run.add_argument(
        "--agent",
        default="claude",
        choices=("claude", "codex"),
        help="agent under test for the matrix (codex backend not implemented yet)",
    )
    run.add_argument("--claude-bin", default="claude", help="`claude` binary (default: PATH)")
    run.add_argument("--model", default="opus", help="model for the agent under test")
    run.add_argument(
        "--effort",
        default="high",
        choices=("high", "xhigh", "max"),
        help="reasoning effort (never fast/low for an eval; default: high)",
    )
    run.add_argument("--judge-model", default=DEFAULT_JUDGE_MODEL, help="LLM judge model")
    run.add_argument("--timeout", type=float, default=600.0, help="per-rollout seconds")
    run.add_argument(
        "--system-prompt-file", type=Path, default=None, help="prompt text file to test"
    )
    run.add_argument(
        "--system-prompt-nix", type=Path, default=None, help="render this .nix and test it"
    )
    run.add_argument(
        "--json-out",
        type=Path,
        default=None,
        help="write the JSON report here (the machine output; view it with the viewer app)",
    )
    run.add_argument(
        "--fail-under", type=float, default=None, help="exit 1 if any eval's headline below"
    )
    run.add_argument(
        "--streak",
        type=int,
        default=None,
        help="exit 1 unless behaviors hits this many consecutive all-pass rollouts",
    )
    return parser


def _git_rev() -> str | None:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError:
        return None
    return (proc.stdout.strip() or None) if proc.returncode == 0 else None


def _selected(name: str) -> list[str]:
    return list(EVALS.keys()) if name == "all" else [name]


def _run_one(name: str, ctx: EvalContext) -> EvalReport:
    if name == behaviors_eval.NAME:
        return behaviors_eval.run(ctx)
    if name == first_principles_eval.NAME:
        return first_principles_eval.run(ctx)
    if name == reverse_engineering_eval.NAME:
        return reverse_engineering_eval.run(ctx)
    raise ValueError(f"unknown eval: {name}")


def _run(args: argparse.Namespace) -> int:
    if args.agent != "claude":
        # The matrix seam exists, but only the claude backend is wired so far.
        print(
            f"agent '{args.agent}' is not implemented yet; only --agent claude works. "
            "See the codex-backend tracking issue. Codex shares the same house "
            "system prompt (packages/agent/prompt.nix), so this is the next backend.",
            file=sys.stderr,
        )
        return 2
    prompt_file, prompt_sha = resolve_prompt(args.system_prompt_file, args.system_prompt_nix)
    ctx = EvalContext(
        prompt_file=prompt_file,
        judge=Judge(model=args.judge_model),
        agent_kind=args.agent,
        claude_bin=args.claude_bin,
        model=args.model,
        effort=args.effort,
        rollouts=args.rollouts,
        max_workers=args.max_workers,
        live=args.live,
        sandbox=args.sandbox,
        timeout=args.timeout,
        limit=args.limit,
        progress=_progress,
    )

    reports: list[EvalReport] = []
    for name in _selected(args.eval):
        _progress(f"== eval: {name} ==")
        reports.append(_run_one(name, ctx))

    for rep in reports:
        print(f"\n== {rep.name} ==")
        print(rep.table)

    metadata: dict[str, object] = {
        "prompt_sha256": prompt_sha,
        "git_rev": _git_rev(),
        "timestamp": dt.datetime.now(dt.UTC).isoformat(),
        "agent_model": args.model,
        "effort": args.effort,
        "judge_model": args.judge_model,
        "live": args.live,
        "sandbox": args.sandbox,
        "rollouts": args.rollouts,
    }
    full = {
        "metadata": metadata,
        "evals": {rep.name: rep.to_json() for rep in reports},
    }
    # JSON is the one output (the machine version). Rendering is the viewer app's
    # job: `nix run .#system-prompt-eval-viewer -- <this file>`. Always write a
    # file so there is something to open, defaulting to a temp path.
    json_out = args.json_out or (Path(tempfile.mkdtemp(prefix="sp-eval-")) / "result.json")
    json_out.parent.mkdir(parents=True, exist_ok=True)
    json_out.write_text(json.dumps(full, indent=2), encoding="utf-8")
    print(f"\nwrote {json_out}", file=sys.stderr)
    print(f"view it: nix run .#system-prompt-eval-viewer -- {json_out}", file=sys.stderr)

    rc = 0
    if args.fail_under is not None:
        for rep in reports:
            if rep.headline < args.fail_under:
                print(f"FAIL: {rep.name} headline {rep.headline:.2f} < {args.fail_under}", file=sys.stderr)
                rc = 1
    if args.streak is not None:
        for rep in reports:
            if rep.longest_streak is not None and rep.longest_streak < args.streak:
                print(f"FAIL: {rep.name} streak {rep.longest_streak} < {args.streak}", file=sys.stderr)
                rc = 1
    return rc


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    if args.command == "list":
        for name, desc in EVALS.items():
            print(f"{name:<18} {desc}")
        return 0
    if args.command == "run":
        return _run(args)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
