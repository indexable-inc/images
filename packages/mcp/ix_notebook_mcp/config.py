"""Immutable server configuration, decided once by the CLI.

The CLI builds a :class:`Config`, stashes it via :func:`set_config`, and launches
the Jupyter Server; the in-process extension reads it back with :func:`config` to
construct the :class:`~ix_notebook_mcp.app.NotebookApp`. A module-level handoff is
the simplest correct way to pass values across that boundary, because Jupyter
constructs the extension itself and gives us no place to inject constructor args.
The object is frozen so nothing mutates configuration after launch.
"""

from __future__ import annotations

import os
import stat
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class Config:
    # Directory notebooks live in and the Jupyter Server is rooted at.
    workdir: Path

    # The Jupyter Server bind address. The server runs with auth disabled (no
    # token, no password): the lab URL opens straight into the live notebook so a
    # human can co-edit without copying a token out of band. Access is instead
    # gated by reachability: the CLI default is this node's Tailscale IPv4 when
    # Tailscale is up (so the tailnet is the trust boundary and a phone or
    # laptop on the tailnet can open the lab URL without ssh), and loopback
    # otherwise. The dataclass default stays loopback so an in-process Config()
    # in a test cannot accidentally bind to a tailnet address; the CLI overrides
    # this when it builds the real Config. Note a reachable Jupyter Server is
    # arbitrary code execution for whoever can dial it, so never bind it to a
    # non-tailnet public interface without an explicit IX_MCP_HOST override.
    host: str = "127.0.0.1"
    jupyter_port: int = 0

    # The host string put into the lab URL a human opens. Distinct from `host`
    # (the bind address): when Jupyter binds a wildcard like 0.0.0.0, that is
    # not a usable URL host, so the CLI resolves a reachable name (tailnet, fqdn)
    # and stores it here. Defaults to the loopback bind so behaviour is unchanged.
    advertised_host: str = "127.0.0.1"

    # "stdio" (the default; what an MCP client launches) or "http".
    transport: str = "stdio"

    # When transport == "http", the MCP endpoint binds here. Distinct from the
    # Jupyter port above: both run in this process and cannot share a port.
    mcp_http_host: str = "127.0.0.1"
    mcp_http_port: int = 8000

    # In stdio mode the CLI dups the real stdin/stdout to these fds before the
    # Jupyter Server can write logs to fd 1, so the MCP protocol owns them
    # exclusively. None in http mode.
    stdin_fd: int | None = None
    stdout_fd: int | None = None

    def lab_url(self) -> str:
        """The URL a human opens to co-edit (auth is disabled, so no token)."""
        return f"http://{self.advertised_host}:{self.jupyter_port}/lab"

    def resolve(self, rel_path: str) -> Path:
        """Resolve a workspace-relative path to an absolute one, refusing escapes.

        ``.resolve()`` collapses ``..`` and symlinks first, so a path that points
        outside the workspace (relative, absolute, or via a symlink) is rejected
        rather than silently honoured.
        """
        candidate = (self.workdir / rel_path).resolve()
        workdir = self.workdir.resolve()
        if workdir != candidate and workdir not in candidate.parents:
            raise ValueError(f"path {rel_path!r} escapes the notebook workspace")
        return candidate

    def canonical(self, rel_path: str) -> str:
        """The one canonical workspace-relative spelling of ``rel_path``.

        The YDoc room and the kernel session are both keyed on this string, so
        two spellings of the same file (``x.ipynb`` vs ``./a/../x.ipynb``) must
        collapse to one key or the agent and the human would land on different
        rooms/kernels for the same notebook.
        """
        return self.resolve(rel_path).relative_to(self.workdir.resolve()).as_posix()


_CONFIG: Config | None = None


def set_config(value: Config) -> None:
    global _CONFIG
    _CONFIG = value


def config() -> Config:
    if _CONFIG is None:
        raise RuntimeError("config is unset; ix-mcp must be launched via its CLI")
    return _CONFIG


def runtime_dir() -> Path:
    """A private writable directory for the lab-url handoff file and the
    materialized JupyterLab config (custom CSS + settings) served to the
    authenticated browser.

    Hardened against an untrusted base: when neither ``XDG_RUNTIME_DIR`` nor
    ``TMPDIR`` is set (e.g. a headless service) the base is the world-writable
    sticky ``/tmp``, where another local user could pre-create ``ix-mcp`` and drop
    files we would then serve into the session (CWE-377). So create it ``0700`` and
    fail closed if a pre-existing one is a symlink, not ours, or group/other
    accessible, rather than silently reusing it.
    """
    base = os.environ.get("XDG_RUNTIME_DIR") or os.environ.get("TMPDIR") or "/tmp"
    path = Path(base) / "ix-mcp"
    # mode is masked by umask on create and ignored when the dir already exists,
    # so perms are re-asserted (and ownership checked) below regardless.
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        raise RuntimeError(f"runtime dir {path} is not a real directory")
    if info.st_uid != os.getuid():
        raise RuntimeError(f"runtime dir {path} is not owned by the current user")
    if info.st_mode & 0o077:
        path.chmod(0o700)
    return path
