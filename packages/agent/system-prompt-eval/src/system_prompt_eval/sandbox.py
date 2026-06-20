"""Cross-platform process sandboxing for the patched-repo eval.

When ``--sandbox`` is set, an agent rollout is wrapped so its filesystem WRITES
are confined to a throwaway sandbox root (it cannot touch the real checkout or
the user's files), while reads and network stay open: the agent still needs to
read the Nix store to run and reach the Anthropic API to think. The point of the
eval is to confine the agent to a single patched repo and protect the host, not
to airtight-isolate compute (that is what ix VMs are for, see search-eval's
IxVmBackend).

Backends, chosen by platform:

- macOS: ``sandbox-exec`` (Seatbelt). A generated profile allows everything, then
  denies ``file-write*`` outside the sandbox root and the standard temp dirs. The
  agent's ``HOME`` is pointed inside the root so Claude Code's own writes land in
  the allowed area.
- Linux: ``bwrap`` (bubblewrap) if present: a read-only bind of ``/`` with a
  fresh writable tmpfs for the sandbox root and ``/tmp``, sharing the network.

If ``--sandbox`` is requested and no backend is available, this raises rather
than silently running unsandboxed (no silent fallback).
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path


class SandboxError(RuntimeError):
    """A sandbox was requested but could not be constructed."""


def available_backend() -> str | None:
    """Name of the sandbox backend usable on this host, or None."""
    if sys.platform == "darwin" and shutil.which("sandbox-exec"):
        return "sandbox-exec"
    if sys.platform.startswith("linux") and shutil.which("bwrap"):
        return "bwrap"
    return None


def wrap(args: list[str], *, root: Path) -> list[str]:
    """Wrap ``args`` so writes are confined to ``root``. Reads/network stay open."""
    backend = available_backend()
    if backend is None:
        raise SandboxError(
            "no sandbox backend on this host: need `sandbox-exec` (macOS) or "
            "`bwrap` (Linux). Re-run without --sandbox to skip OS isolation."
        )
    if backend == "sandbox-exec":
        return _seatbelt(args, root=root)
    return _bwrap(args, root=root)


def _seatbelt(args: list[str], *, root: Path) -> list[str]:
    # Allow-by-default, then deny writes outside the sandbox root and temp dirs.
    profile = "\n".join(
        [
            "(version 1)",
            "(allow default)",
            "(deny file-write*)",
            "(allow file-write*",
            f'  (subpath "{root}")',
            '  (subpath "/private/tmp")',
            '  (subpath "/private/var/folders")',
            '  (subpath "/tmp")',
            '  (literal "/dev/null")',
            '  (literal "/dev/stdout")',
            '  (literal "/dev/stderr")',
            "  (subpath \"/dev/fd\"))",
            "(allow network*)",
        ]
    )
    return ["sandbox-exec", "-p", profile, *args]


def _bwrap(args: list[str], *, root: Path) -> list[str]:
    return [
        "bwrap",
        "--ro-bind",
        "/",
        "/",
        "--tmpfs",
        "/tmp",  # noqa: S108 - a tmpfs mount point inside the sandbox, not a host temp path
        "--bind",
        str(root),
        str(root),
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--share-net",
        "--die-with-parent",
        *args,
    ]
