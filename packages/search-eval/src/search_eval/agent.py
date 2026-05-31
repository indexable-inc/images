"""Tier B: drive a headless `claude -p` agent whose only tool is search.

This measures the downstream question Exa frames as RAG/SimpleQA: does our search
actually let an agent answer a question it cannot answer from memory? Each task
runs an isolated `claude -p` whose *only* capability is a corpus-scoped search
command, so a correct answer is evidence the retrieval surfaced the fact.

Isolation is a pluggable backend:

- [`LocalBackend`] runs the agent in a fresh empty temp directory with a
  one-tool MCP search server (see [`mcp_server`]) and Bash plus every file/web
  reader denied, and keeps the corpus path out of the agent's view (the MCP
  config lives outside the working directory and the prompt never names a path).
  This is **best-effort isolation for a cooperative agent**, not a security
  boundary: Claude Code still executes read-only shell regardless of the tool
  allow/deny lists, so an adversarial agent that goes hunting on the filesystem
  could read the corpus without searching. It is the default for local runs.
- [`IxVmBackend`] is the typed seam for the **airtight** boundary: run each
  agent inside a disposable ix VM whose only view of the corpus is the search
  tool (the same pattern Symphony uses for Codex). It is not wired up yet: ix
  VMs run on x86_64-linux compute nodes, so this backend is implemented as an
  explicit, observable error rather than a silent fallback to local.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

from .model import TaskCase

# The MCP tool id Claude Code exposes for the `search` tool on the `corpus`
# server (see mcp_server.py): `mcp__<server>__<tool>`.
_SEARCH_TOOL = "mcp__corpus__search"

# Tools the agent must not have, so the only way to read the corpus is the MCP
# search tool. Bash is denied (a Bash allowlist does not reliably restrict
# commands), along with every direct file/web reader.
_DENIED_TOOLS = "Bash,Read,Grep,Glob,Edit,Write,WebSearch,WebFetch,Task,NotebookEdit"

_PROMPT = """\
You are answering a question about a small codebase. You have exactly ONE tool: \
a semantic `search` over the codebase that returns matching files with their \
contents. You cannot read, list, or grep files any other way.

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
        # Two temp dirs: an empty working directory for the agent, and a separate
        # config directory the agent's cwd cannot see, so `ls`/`cat` in the cwd
        # never reveals the corpus path that the MCP config carries.
        with (
            tempfile.TemporaryDirectory(prefix="search-eval-cwd-") as cwd,
            tempfile.TemporaryDirectory(prefix="search-eval-cfg-") as cfg_dir,
        ):
            config = Path(cfg_dir) / "mcp.json"
            config.write_text(self._mcp_config(), encoding="utf-8")
            args = [
                self.claude_bin,
                "-p",
                _PROMPT.format(task=case.task),
                "--output-format",
                "json",
                "--mcp-config",
                str(config),
                "--allowedTools",
                _SEARCH_TOOL,
                "--disallowedTools",
                _DENIED_TOOLS,
            ]
            if self.agent_model:
                args += ["--model", self.agent_model]
            return self._invoke(args, cwd=cwd, env=dict(os.environ))

    def _mcp_config(self) -> str:
        # Spawn the one-tool MCP search server with this interpreter so it shares
        # the installed `search_eval` + `mcp` packages; `search` resolves from
        # the inherited PATH. Corpus and scope travel via the environment.
        return json.dumps(
            {
                "mcpServers": {
                    "corpus": {
                        "command": sys.executable,
                        "args": ["-m", "search_eval.mcp_server"],
                        "env": {
                            "SEARCH_EVAL_CORPUS": str(self.corpus),
                            "SEARCH_EVAL_SEARCH_BIN": self.search_bin,
                            "SEARCH_EVAL_MAX_RESULTS": str(self.max_results),
                        },
                    }
                }
            }
        )

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
