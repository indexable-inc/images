"""Immutable server configuration, decided once by the CLI and read back by the
kernel manager, dashboard, and MCP transport.

The CLI builds a :class:`Config`, stashes it via :func:`set_config`, and runs the
event loop; the other modules read it with :func:`config`. A module-level handoff
keeps the wiring simple. The object is frozen so nothing mutates after launch.
"""

from __future__ import annotations

import os
import stat
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Config:
    # Directory the kernel runs in and notebooks/files are resolved against.
    workdir: Path

    # The dashboard HTTP bind address. The dashboard is read-only (it renders the
    # execution store), but the store can contain anything the agent ran, so the
    # default bind is this node's Tailscale IPv4 when Tailscale is up (tailnet is
    # the trust boundary) and loopback otherwise. IX_MCP_HOST overrides.
    host: str = "127.0.0.1"
    dashboard_port: int = 0

    # Host advertised in the dashboard URL (distinct from the bind: a wildcard
    # bind is not a usable URL host, so the CLI resolves a reachable name).
    advertised_host: str = "127.0.0.1"

    # Path to the SQLite execution store the kernel writes and the dashboard reads.
    store_path: Path | None = None

    # "stdio" (the default; what an MCP client launches) or "http".
    transport: str = "stdio"
    mcp_http_host: str = "127.0.0.1"
    mcp_http_port: int = 8000

    # In stdio mode the CLI dups the real stdin/stdout to these fds before any
    # library can write to fd 1, so the MCP protocol owns them exclusively.
    stdin_fd: int | None = None
    stdout_fd: int | None = None

    # Seconds past a cell's own ``budget`` that the server waits for the kernel to
    # report idle before treating it as wedged by a synchronous call, interrupting
    # the kernel, and returning an actionable summary. See ``kernel.python_exec``.
    wedge_grace: float = 15.0

    def dashboard_url(self) -> str:
        return f"http://{self.advertised_host}:{self.dashboard_port}/"

    def resolve(self, rel_path: str) -> Path:
        candidate = (self.workdir / rel_path).resolve()
        workdir = self.workdir.resolve()
        if workdir != candidate and workdir not in candidate.parents:
            raise ValueError(f"path {rel_path!r} escapes the workspace")
        return candidate


_CONFIG: Config | None = None


def set_config(value: Config) -> None:
    global _CONFIG
    _CONFIG = value


def config() -> Config:
    if _CONFIG is None:
        raise RuntimeError("config is unset; ix-mcp must be launched via its CLI")
    return _CONFIG


def runtime_dir() -> Path:
    """A private 0700 writable dir for the store and the dashboard-url handoff.

    Hardened against an untrusted shared base (/tmp): fail closed if a
    pre-existing one is a symlink, not ours, or group/other accessible (CWE-377).
    """
    base = os.environ.get("XDG_RUNTIME_DIR") or os.environ.get("TMPDIR") or "/tmp"
    path = Path(base) / "ix-mcp"
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise RuntimeError(f"runtime dir {path} is not a real directory")
    if info.st_uid != os.getuid():
        raise RuntimeError(f"runtime dir {path} is not owned by the current user")
    if info.st_mode & 0o077:
        path.chmod(0o700)
    return path
