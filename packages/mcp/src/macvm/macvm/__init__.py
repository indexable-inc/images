"""Native macOS VM control for the ix-mcp interpreter.

Bundled into the pinned interpreter the same way ``tui``, ``search`` and
``screen`` are, so every session can ``import macvm`` with no setup. Where
``screen`` captures the host desktop, ``macvm`` boots a guest VM and captures
*its* display, fully off-screen: nothing appears on the host desktop and the
host cursor is never touched. This is the way to verify on-screen rendering (a
GUI app, a boot screen) inside an isolated VM without taking over the machine.

    import macvm
    print(macvm.info())                       # is virtualization available?
    img = macvm.screenshot("/path/to/guest")  # boot the guest, return a PIL.Image
    img                                        # auto-rendered inline by python_eval/exec

A guest is a *bundle* directory (``disk.img``, ``aux.img``,
``hardware-model.bin``, ``machine-id.bin``) created once with
:func:`install`::

    macvm.install("/path/to/UniversalMac_26.5_Restore.ipsw", "/path/to/guest")

The work is done by the ``macos-vm`` binary (a thin Rust binding over Apple's
Virtualization.framework). It holds the ``com.apple.security.virtualization``
entitlement by self-signing into a per-user cache on first use, so no manual
``codesign`` is needed. The capture reads the guest framebuffer IOSurface
directly, so it needs no Screen-Recording permission.

This module is macOS-only: it raises on a non-Darwin platform, and the
``macos-vm`` binary is only bundled into the interpreter on Darwin.
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from PIL import Image

__all__ = ["MacVmError", "info", "install", "screenshot"]


class MacVmError(RuntimeError):
    """A macos-vm invocation failed, or the platform/binary is unavailable."""


def _binary() -> str:
    if sys.platform != "darwin":
        raise MacVmError("macvm is macOS-only")
    path = os.environ.get("IX_MACVM_BIN")
    if not path:
        raise MacVmError(
            "IX_MACVM_BIN is not set; the macos-vm binary is bundled into ix-mcp "
            "on Darwin only"
        )
    return path


def info() -> str:
    """Return whether Virtualization.framework can run a VM on this host."""
    out = subprocess.run(
        [_binary(), "info"], capture_output=True, text=True, check=False
    )
    return (out.stdout or out.stderr).strip()


def install(ipsw: str | os.PathLike, bundle: str | os.PathLike, disk_gib: int = 64, timeout: float = 2400) -> None:
    """Install macOS into a fresh ``bundle`` directory from a local ``ipsw``.

    Takes ~15-20 minutes. Raises :class:`MacVmError` on failure.
    """
    try:
        result = subprocess.run(
            [_binary(), "install-macos", "--ipsw", str(ipsw), "--bundle", str(bundle), "--disk-gib", str(disk_gib)],
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        raise MacVmError(f"install-macos timed out after {timeout}s") from exc
    if result.returncode != 0:
        raise MacVmError(f"install-macos failed: {result.stderr.strip()}")


def screenshot(bundle: str | os.PathLike, seconds: int = 20, timeout: float | None = None) -> "Image.Image":
    """Boot the macOS guest in ``bundle`` off-screen and return a ``PIL.Image``
    of its display after ``seconds`` (the last frame captured).

    Raises :class:`MacVmError` if the binary fails, times out, or produces no
    frame.
    """
    from PIL import Image

    bin_path = _binary()
    deadline = timeout if timeout is not None else seconds + 120
    with tempfile.TemporaryDirectory(prefix="ix-macvm-") as tmp:
        prefix = pathlib.Path(tmp) / "shot"
        try:
            result = subprocess.run(
                [bin_path, "boot-macos", "--bundle", str(bundle), "--out-prefix", str(prefix), "--seconds", str(seconds)],
                capture_output=True,
                text=True,
                check=False,
                timeout=deadline,
            )
        except subprocess.TimeoutExpired as exc:
            raise MacVmError(f"boot-macos timed out after {deadline}s") from exc
        shots = sorted(pathlib.Path(tmp).glob("shot.*.png"))
        if not shots:
            raise MacVmError(
                f"boot-macos produced no screenshot (rc={result.returncode}): {result.stderr.strip()}"
            )
        # Load and detach from the temp file before it is removed.
        with Image.open(shots[-1]) as img:
            return img.convert("RGB")
