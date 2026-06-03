"""Process-global runtime state shared between the CLI, the Jupyter extension,
and the MCP tools.

These live in one module because all three run in the *same* process (that
co-location is what makes real-time co-editing possible), so a module-level
singleton is the simplest correct way to hand the extension the values the CLI
chose before it launched the Jupyter Server.
"""

from __future__ import annotations

import os
import secrets
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class Runtime:
    """Everything the extension and tools need that the CLI decides up front."""

    # Where notebooks live and the Jupyter Server is rooted. A human opens
    # notebooks from here; the agent creates them here.
    workdir: Path = field(default_factory=lambda: Path.cwd())

    # The Jupyter Server bind host/port and auth token. The token gates both the
    # browser UI and the collaboration websocket, so the same value is printed in
    # the lab URL a human opens.
    host: str = "127.0.0.1"
    port: int = 0
    token: str = field(default_factory=lambda: secrets.token_urlsafe(24))

    # "stdio" (the default; what Claude Code / Codex launch) or "http".
    transport: str = "stdio"

    # When transport == "http", the MCP endpoint binds here. This is distinct from
    # the Jupyter Server's own port (above): both run in this process, so they
    # cannot share a port. The Jupyter Server always takes a free random port; the
    # MCP HTTP endpoint takes the address the user asked for.
    mcp_http_host: str = "127.0.0.1"
    mcp_http_port: int = 8000

    # The real stdout file descriptor, duplicated before the Jupyter Server can
    # write logs to it. The MCP stdio protocol owns this fd exclusively; fd 1 in
    # the process is redirected to stderr so a stray ``print`` from a library
    # cannot corrupt the JSON-RPC stream. Set by the CLI before launch.
    mcp_stdout_fd: int | None = None
    mcp_stdin_fd: int | None = None

    # Bound to the live Jupyter ServerApp once the extension starts, so tools can
    # reach the kernel manager, session manager, and YDoc rooms.
    serverapp: Any = None

    # The notebook path (relative to ``workdir``) most recently opened via
    # ``notebook_use``; the default target for cell operations that omit a path.
    active_notebook: str | None = None

    def lab_url(self) -> str:
        """The URL a human opens to co-edit, including the auth token."""
        return f"http://{self.host}:{self.port}/lab?token={self.token}"

    def abspath(self, rel_path: str) -> Path:
        """Resolve a notebook path against the workspace, refusing escapes."""
        candidate = (self.workdir / rel_path).resolve()
        workdir = self.workdir.resolve()
        if workdir not in candidate.parents and candidate != workdir:
            raise ValueError(f"path {rel_path!r} escapes the notebook workspace")
        return candidate


RUNTIME = Runtime()


def runtime_dir() -> Path:
    """A writable directory for the lab-url handoff file, so ``ix-mcp lab`` (a
    second process) can find the running server's URL. Uses ``XDG_RUNTIME_DIR``
    when set, else a temp dir; created on demand."""
    base = os.environ.get("XDG_RUNTIME_DIR") or os.environ.get("TMPDIR") or "/tmp"
    path = Path(base) / "ix-mcp"
    path.mkdir(parents=True, exist_ok=True)
    return path
