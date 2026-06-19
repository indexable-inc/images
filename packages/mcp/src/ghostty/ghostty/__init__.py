"""Drive the Ghostty terminal from the ix-mcp interpreter (macOS only).

Bundled like ``screen``/``maps``/``imessage`` so every session can ``import
ghostty`` on Darwin with no install step. Ghostty 1.3.2+ exposes an AppleScript
dictionary whose ``terminal`` surfaces carry ``id``/``tty``/``pid``/``working
directory``/``name``; this module reads those into a polars frame and lets you
close, focus, or activate a surface.

    import ghostty
    await ghostty.surfaces()          # every open surface as a polars frame
    await ghostty.my_tty()            # the tty this very session runs on
    await ghostty.close_me()          # close the window this session lives in

The marquee use is ``close_me``: an agent that has *fully* finished its work can
shut its own window. It resolves the session's controlling tty by walking the
kernel process up to its claude/login ancestor (the kernel is a child of the CLI
that launched it), matches the Ghostty surface whose ``tty`` equals it, and
closes that surface. The match is exact (``/dev/ttysNNN``), so it never touches a
sibling session sharing the app.

Why AppleScript over a subprocess of ``ghostty`` the binary: Ghostty has no CLI
to enumerate or close surfaces; the scripting dictionary is the only supported
control surface. Each call shells ``osascript`` on the event loop via
``asyncio.create_subprocess_exec`` (never the blocking ``subprocess.run``), so a
co-running coroutine on the shared kernel is never frozen.

macOS-only: importing on a non-Darwin platform raises ``RuntimeError``.
"""

from __future__ import annotations

import asyncio
import os
import sys
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import polars as pl

__all__ = [
    "GhosttyError",
    "activate",
    "close",
    "close_me",
    "focus",
    "is_running",
    "my_surface",
    "my_tty",
    "surfaces",
]

__version__ = "0.1.0"

if sys.platform != "darwin":
    raise RuntimeError(
        "ghostty: the Ghostty AppleScript control surface is macOS-only "
        f"(running on {sys.platform!r})."
    )

# AppleScript control-character separators. Surface titles and working
# directories can contain spaces, tabs, even pipes, but never the ASCII unit/
# record separators (US=31, RS=30), so they delimit the readout unambiguously.
_FS = "\x1f"
_RS = "\x1e"

# The five terminal properties surfaces() reads, in column order. `tty` is the
# Ghostty 1.3.2+ addition this whole module hinges on.
_FIELDS = ("id", "tty", "pid", "working_directory", "name")


class GhosttyError(RuntimeError):
    """A Ghostty AppleScript call failed, or no surface matched the request."""


async def _osascript(script: str) -> str:
    """Run one AppleScript and return its stdout, raising GhosttyError on failure."""
    proc = await asyncio.create_subprocess_exec(
        "osascript",
        "-e",
        script,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    out, err = await proc.communicate()
    if proc.returncode != 0:
        raise GhosttyError(
            f"osascript failed (exit {proc.returncode}): "
            f"{err.decode(errors='replace').strip()}"
        )
    return out.decode(errors="replace")


async def is_running() -> bool:
    """True when Ghostty is already running.

    Checked first by every call that would otherwise ``tell application
    "Ghostty"`` and silently launch the app. ``is running`` does not launch it.
    """
    out = await _osascript('tell application "System Events" to (name of processes) contains "Ghostty"')
    return out.strip() == "true"


async def _ps_tree() -> dict[int, tuple[int, str]]:
    """Snapshot the process table as ``pid -> (ppid, tty)`` in one ``ps`` call.

    One subprocess, parsed in Python, beats a ``ps`` per ancestor: the walk in
    :func:`my_tty` then never touches the loop again.
    """
    proc = await asyncio.create_subprocess_exec(
        "ps", "-Ao", "pid=,ppid=,tty=", stdout=asyncio.subprocess.PIPE
    )
    out, _ = await proc.communicate()
    tree: dict[int, tuple[int, str]] = {}
    for line in out.decode(errors="replace").splitlines():
        parts = line.split(None, 2)
        if len(parts) < 2:
            continue
        pid_s, ppid_s = parts[0], parts[1]
        tty = parts[2].strip() if len(parts) == 3 else ""
        try:
            tree[int(pid_s)] = (int(ppid_s), tty)
        except ValueError:
            continue
    return tree


async def my_tty() -> str | None:
    """The controlling tty of this session, e.g. ``"/dev/ttys000"``, or None.

    The kernel runs as a child of the claude/codex process that launched the MCP
    server, which is itself attached to the Ghostty surface's pty. So walk the
    parent chain from this process and return the first ancestor bound to a real
    ``ttysNNN`` device. ``ps`` prints the tty as ``ttys000``; Ghostty's
    AppleScript ``tty`` reports ``/dev/ttys000``, so normalise to the latter.
    """
    tree = await _ps_tree()
    pid: int | None = os.getpid()
    seen: set[int] = set()
    while pid is not None and pid not in seen:
        seen.add(pid)
        ppid, tty = tree.get(pid, (0, ""))
        if tty.startswith("ttys"):
            return f"/dev/{tty}"
        pid = ppid or None
    return None


def _surface_schema() -> dict[str, type[str] | type[int]]:
    """Column dtypes for the surfaces() frame (pid is the only integer)."""
    return {f: (int if f == "pid" else str) for f in _FIELDS}


async def surfaces() -> pl.DataFrame:
    """Every open Ghostty surface as a polars frame.

    Columns: ``id``, ``tty``, ``pid``, ``working_directory``, ``name``. Empty
    frame (correct schema) when Ghostty is not running, so callers can filter
    without a special case.
    """
    import polars as pl

    schema = _surface_schema()
    if not await is_running():
        return pl.DataFrame(schema=schema)
    script = (
        'tell application "Ghostty"\n'
        f"  set fs to (ASCII character 31)\n"
        f"  set rs to (ASCII character 30)\n"
        '  set out to ""\n'
        "  repeat with s in terminals\n"
        "    set out to out & (id of s) & fs & (tty of s) & fs & (pid of s) & fs"
        " & (working directory of s) & fs & (name of s) & rs\n"
        "  end repeat\n"
        "  return out\n"
        "end tell"
    )
    raw = await _osascript(script)
    rows: list[dict[str, object]] = []
    for record in raw.split(_RS):
        if not record.strip():
            continue
        cells = record.split(_FS)
        if len(cells) != len(_FIELDS):
            continue
        row: dict[str, object] = dict(zip(_FIELDS, (c.strip() for c in cells)))
        try:
            row["pid"] = int(str(row["pid"]))
        except ValueError:
            row["pid"] = None
        rows.append(row)
    return pl.DataFrame(rows, schema=schema) if rows else pl.DataFrame(schema=schema)


async def my_surface() -> pl.DataFrame:
    """The single-row frame for this session's own surface (empty if unmatched)."""
    tty = await my_tty()
    frame = await surfaces()
    if tty is None:
        return frame.clear()
    return frame.filter(frame["tty"] == tty)


def _selector(*, tty: str | None, id: str | None) -> str:
    """The AppleScript ``whose`` clause selecting one terminal by tty or id."""
    if tty is not None:
        return f'(first terminal whose tty is "{tty}")'
    if id is not None:
        return f'(first terminal whose id is "{id}")'
    raise GhosttyError("pass exactly one of tty= or id=")


async def _command(verb: str, *, tty: str | None, id: str | None) -> str:
    """Run a single-terminal command (``close`` / ``focus`` / ``activate window``)."""
    if not await is_running():
        raise GhosttyError("Ghostty is not running")
    sel = _selector(tty=tty, id=id)
    script = (
        'tell application "Ghostty"\n'
        f"  set s to {sel}\n"
        f"  {verb} s\n"
        "end tell"
    )
    await _osascript(script)
    return f"{verb}: {tty or id}"


async def close(*, tty: str | None = None, id: str | None = None) -> str:
    """Close one terminal surface, selected by ``tty`` or by ``id``.

    Closing the last surface in a window closes the window. Pass exactly one of
    ``tty`` / ``id``.
    """
    return await _command("close", tty=tty, id=id)


async def focus(*, tty: str | None = None, id: str | None = None) -> str:
    """Focus one terminal surface (and bring its window forward)."""
    return await _command("focus", tty=tty, id=id)


async def activate(*, tty: str | None = None, id: str | None = None) -> str:
    """Bring the window owning a terminal surface to the front."""
    if not await is_running():
        raise GhosttyError("Ghostty is not running")
    sel = _selector(tty=tty, id=id)
    script = (
        'tell application "Ghostty"\n'
        f"  activate window of {sel}\n"
        "end tell"
    )
    await _osascript(script)
    return f"activate: {tty or id}"


async def close_me() -> str:
    """Close the Ghostty surface this session is running in.

    Resolves :func:`my_tty` and closes the matching surface, ending the session.
    The deliberate end-of-task move for an agent that is fully done.
    """
    tty = await my_tty()
    if tty is None:
        raise GhosttyError(
            "could not resolve this session's tty (no ttys* ancestor); "
            "not running under a Ghostty pty?"
        )
    return await close(tty=tty)
