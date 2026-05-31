"""Tier B: drive a headless `claude -p` agent whose only tool is search.

This measures the downstream question Exa frames as RAG/SimpleQA: does our search
actually let an agent answer a question it cannot answer from memory? Each task
runs an isolated `claude -p` whose *only* capability is a corpus-scoped search
command, so a correct answer is evidence the retrieval surfaced the fact.

Isolation is a pluggable backend:

- [`LocalBackend`] runs the agent in a fresh empty temp directory with a
  generated ``corpus-search`` wrapper on PATH. The empty cwd plus a tool
  allowlist means the agent cannot read or grep the corpus directly; it must
  search. This is the default and is what runs in CI-style local runs.
- [`IxVmBackend`] is the typed seam for running each agent inside a disposable
  ix VM (the production isolation boundary, the same pattern Symphony uses for
  Codex). It is not wired up yet: ix VMs run on x86_64-linux compute nodes, so
  this backend is implemented as an explicit, observable error rather than a
  silent fallback to local.
"""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from .model import TaskCase

_PROMPT = """\
You are answering a question about a small codebase. You have exactly ONE tool: \
the shell command `corpus-search "<query>"`, which runs a semantic search over \
the codebase and prints matching files with their contents. You cannot read or \
list files any other way.

Question: {task}

Search as many times as you need, then end with a single final line of the form:
ANSWER: <your concise answer>"""


class AgentError(RuntimeError):
    """The agent process failed to produce a usable answer."""


def _extract_answer(text: str) -> str:
    """Pull the ``ANSWER:`` line if present, else return the trimmed text."""
    for line in reversed(text.splitlines()):
        stripped = line.strip()
        if stripped.upper().startswith("ANSWER:"):
            return stripped[len("ANSWER:") :].strip()
    return text.strip()


@dataclass(frozen=True, slots=True)
class LocalBackend:
    """Run each agent locally in a throwaway sandbox directory."""

    corpus: Path
    search_bin: str = "search"
    claude_bin: str = "claude"
    max_results: int = 8
    agent_model: str | None = None
    timeout_seconds: float = 300.0

    def run_task(self, case: TaskCase) -> str:
        with tempfile.TemporaryDirectory(prefix="search-eval-") as sandbox:
            bin_dir = Path(sandbox) / "bin"
            bin_dir.mkdir()
            self._write_wrapper(bin_dir / "corpus-search")
            env = dict(os.environ)
            env["PATH"] = f"{bin_dir}{os.pathsep}{env.get('PATH', '')}"
            args = [
                self.claude_bin,
                "-p",
                _PROMPT.format(task=case.task),
                "--output-format",
                "json",
                "--allowedTools",
                "Bash(corpus-search:*)",
                "--disallowedTools",
                "Read,Grep,Glob,Edit,Write,WebSearch,WebFetch,Task",
            ]
            if self.agent_model:
                args += ["--model", self.agent_model]
            return self._invoke(args, cwd=sandbox, env=env)

    def _write_wrapper(self, path: Path) -> None:
        # The agent calls `corpus-search "<query>"`; the wrapper pins the corpus
        # path, code-only scope, and content display so the model sees snippets.
        path.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f'exec {self.search_bin} --source code -c --no-sync -m {self.max_results} '
            f'"$1" {self.corpus}\n',
            encoding="utf-8",
        )
        path.chmod(0o755)

    def _invoke(self, args: list[str], *, cwd: str, env: dict[str, str]) -> str:
        try:
            proc = subprocess.run(
                args,
                cwd=cwd,
                env=env,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                check=False,
            )
        except FileNotFoundError as exc:
            raise AgentError(f"`{self.claude_bin}` not found on PATH") from exc
        except subprocess.TimeoutExpired as exc:
            raise AgentError(f"agent timed out after {self.timeout_seconds}s") from exc
        if proc.returncode != 0:
            raise AgentError(
                f"claude exited {proc.returncode}: {proc.stderr.strip()[:400] or '(no stderr)'}"
            )
        try:
            envelope = json.loads(proc.stdout)
        except json.JSONDecodeError as exc:
            raise AgentError(f"agent output was not JSON: {proc.stdout[:300]!r}") from exc
        if envelope.get("is_error"):
            raise AgentError(f"agent reported an error: {envelope.get('result', '')[:300]}")
        return _extract_answer(str(envelope.get("result", "")))


@dataclass(frozen=True, slots=True)
class IxVmBackend:
    """Deferred: run each agent inside a disposable ix VM.

    The production isolation boundary. Creating it would: ``ix new`` a VM from a
    claude-code image, mount or clone the corpus in, run the same headless
    ``claude -p`` against an in-VM ``corpus-search``, collect the answer, then
    destroy the VM by id. ix VMs run on x86_64-linux compute nodes, so this
    cannot run from a macOS host and is left as an explicit error.
    """

    def run_task(self, case: TaskCase) -> str:  # noqa: ARG002 - interface stub
        raise AgentError(
            "the ixvm backend is not implemented yet; run with --backend local. "
            "See packages/search-eval/README.md for the design."
        )
