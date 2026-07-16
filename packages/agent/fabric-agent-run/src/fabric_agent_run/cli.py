"""Command-line boundary for a single production Fabric agent call."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import signal
import sys
from pathlib import Path

from .backends import supported_harnesses, validate_provider_credential
from .runner import AgentSpec, Outcome, RunState, run_agent


def _positive_float(value: str) -> float:
    parsed = float(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def _positive_int(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="fabric-agent-run",
        description="Run one Claude or Codex call and record its full lifecycle in Weave.",
    )
    parser.add_argument("--name", required=True, help="workflow name recorded on the task")
    parser.add_argument("--harness", required=True, choices=supported_harnesses())
    parser.add_argument("--model", required=True)
    parser.add_argument("--effort", help="reasoning effort (Codex only)")
    parser.add_argument(
        "--timeout-seconds",
        type=_positive_float,
        default=3600.0,
        help="hard wall-clock deadline (default: 3600)",
    )
    parser.add_argument(
        "--max-result-bytes",
        type=_positive_int,
        default=16 * 1024 * 1024,
        help="maximum final answer size read into memory (default: 16777216)",
    )
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument(
        "--requested-by",
        default=os.environ.get("IX_WEAVE_AGENT") or "fabric-agent-run",
    )
    parser.add_argument(
        "--prompt-file",
        type=argparse.FileType("rb"),
        default=sys.stdin.buffer,
        help="prompt file; default is stdin",
    )
    parser.add_argument(
        "--output",
        choices=("text", "json"),
        default="text",
        help="terminal output format (default: text)",
    )
    return parser


def _install_signal_handlers(stop: asyncio.Event) -> None:
    loop = asyncio.get_running_loop()
    for signum in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(signum, stop.set)


def _emit(outcome: Outcome, output: str) -> None:
    if output == "json":
        print(
            json.dumps(
                {
                    "task": outcome.task,
                    "state": outcome.state.value,
                    "result": outcome.result,
                    "error": outcome.error,
                },
                separators=(",", ":"),
            )
        )
        return
    if outcome.state == RunState.DONE:
        print(outcome.result or "")
    elif outcome.error:
        print(f"fabric-agent-run: {outcome.task}: {outcome.error}", file=sys.stderr)
    else:
        print(f"fabric-agent-run: {outcome.task}: {outcome.state.value}", file=sys.stderr)


async def _run(args: argparse.Namespace) -> Outcome:
    harness: str = args.harness
    validate_provider_credential(harness, os.environ)
    prompt: bytes = args.prompt_file.read()
    if not prompt.strip():
        raise ValueError("prompt is empty")
    cwd: Path = await asyncio.to_thread(args.cwd.resolve, strict=True)
    if not await asyncio.to_thread(cwd.is_dir):
        raise NotADirectoryError(cwd)
    if harness == "claude" and args.effort is not None:
        raise ValueError("--effort is supported only by the codex harness")
    stop = asyncio.Event()
    _install_signal_handlers(stop)
    spec = AgentSpec(
        name=args.name,
        harness=harness,
        model=args.model,
        effort=args.effort,
        prompt=prompt,
        cwd=cwd,
        timeout_seconds=args.timeout_seconds,
        requested_by=args.requested_by,
        max_result_bytes=args.max_result_bytes,
    )
    return await run_agent(spec, stop=stop)


def main() -> None:
    args = _parser().parse_args()
    try:
        outcome = asyncio.run(_run(args))
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"fabric-agent-run: {exc}", file=sys.stderr)
        raise SystemExit(2) from None
    _emit(outcome, args.output)
    exit_code = {
        RunState.DONE: 0,
        RunState.FAILED: 1,
        RunState.INTERRUPTED: 130,
    }[outcome.state]
    raise SystemExit(exit_code)
