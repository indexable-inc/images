"""Filesystem search for the ix-mcp kernel: ``grep`` / ``find`` / ``spotlight``.

Each is backed by a battle-tested CLI (ripgrep / fd / macOS Spotlight) run as a
separate process through the async :mod:`sh` helper, and each returns a
``polars.DataFrame`` — so the human watching the dashboard gets the styled HTML
table while you get a frame to ``.filter`` / ``.sort`` / ``.group_by`` / ``.head``.

    rows = await grep("TODO", "src")           # ripgrep -> path, line_number, col, match, line, abs_offset
    files = await find(ext="py", root="src")   # fd       -> path, name, type, size, mtime
    docs = await spotlight("invoice", "~")     # mdfind   -> path, name, type, size, mtime (macOS only)

Why shell out instead of walking in-process: a parallel recursive walk over a
pathological root saturates CPU either way, but as a *separate process* under
``sh()`` it is bounded by a ``timeout`` (process-group kill) and can be
cancelled — it cannot wedge the kernel's one event loop. The predecessor
(``fff``) walked in-process via a ctypes cdylib and once pinned ~5 cores for an
hour with no way to interrupt short of killing the kernel. Safe defaults keep the
blast radius small: search the cwd, respect ``.gitignore``, cap results, time out.

All three are ``async`` (they shell out), so ``await`` them.
"""

from __future__ import annotations

import json as _json
import os
import stat as _stat
import sys
from datetime import datetime, timezone
from typing import Any

import polars as pl
from sh import sh as _sh  # the bundled async shell-out helper; `sh.sh` is the function

__all__ = ["FsearchError", "find", "grep", "spotlight"]

__version__ = "0.1.0"

DEFAULT_LIMIT = 10_000
DEFAULT_TIMEOUT = 30.0

_GREP_SCHEMA = {
    "path": pl.Utf8,
    "line_number": pl.Int64,
    "col": pl.Int64,
    "match": pl.Utf8,
    "line": pl.Utf8,
    "abs_offset": pl.Int64,
}
_FIND_SCHEMA = {
    "path": pl.Utf8,
    "name": pl.Utf8,
    "type": pl.Utf8,
    "size": pl.Int64,
    "mtime": pl.Datetime(time_zone="UTC"),
}
_KIND_FLAG = {"file": "f", "dir": "d", "symlink": "l"}


class FsearchError(Exception):
    """A search backend exited with an error (or, for spotlight, is unavailable)."""


async def _run(argv: list[str], *, timeout: float, ok_codes: tuple[int, ...] = (0,)):
    """Run a search CLI off the event loop with color disabled (so its output is
    clean, never SGR-corrupted) and surface a non-success exit as FsearchError."""
    out = await _sh(argv, timeout=timeout, color=False)
    if out.code not in ok_codes:
        raise FsearchError(f"{argv[0]} exited {out.code}: {out.text.strip()[:500]}")
    return out


def _lstat_rows(paths: list[str]) -> pl.DataFrame:
    """Turn a list of paths into the find/spotlight frame, one os.lstat per path
    for type/size/mtime (a path that has since vanished gets null metadata)."""
    rows: list[dict[str, Any]] = []
    for p in paths:
        try:
            st = os.lstat(p)
        except OSError:
            kind, size, mtime = None, None, None
        else:
            mode = st.st_mode
            kind = (
                "symlink"
                if _stat.S_ISLNK(mode)
                else "dir"
                if _stat.S_ISDIR(mode)
                else "file"
                if _stat.S_ISREG(mode)
                else "other"
            )
            size = st.st_size
            mtime = datetime.fromtimestamp(st.st_mtime, tz=timezone.utc)
        rows.append(
            {
                "path": p,
                "name": os.path.basename(p.rstrip("/")),
                "type": kind,
                "size": size,
                "mtime": mtime,
            }
        )
    return pl.DataFrame(rows, schema=_FIND_SCHEMA)


async def grep(
    pattern: str,
    root: str | os.PathLike[str] = ".",
    *,
    ignore_case: bool = False,
    fixed: bool = False,
    glob: str | None = None,
    multiline: bool = False,
    hidden: bool = False,
    no_ignore: bool = False,
    limit: int = DEFAULT_LIMIT,
    timeout: float = DEFAULT_TIMEOUT,
) -> pl.DataFrame:
    """Content search via ripgrep, one row per match. Respects ``.gitignore`` by
    default (``no_ignore=True`` to override) and searches ``root`` (cwd by
    default). Columns: ``path, line_number, col, match, line, abs_offset``.
    ``col``/``abs_offset`` are byte offsets. ``fixed`` = literal (no regex)."""
    argv = ["rg", "--json"]
    if ignore_case:
        argv.append("-i")
    if fixed:
        argv.append("-F")
    if multiline:
        argv += ["-U", "--multiline-dotall"]
    if hidden:
        argv.append("--hidden")
    if no_ignore:
        argv.append("--no-ignore")
    if glob:
        argv += ["-g", glob]
    argv += ["--", pattern, os.path.expanduser(str(root))]
    out = await _run(argv, timeout=timeout, ok_codes=(0, 1))  # rg exits 1 on no matches
    rows: list[dict[str, Any]] = []
    for raw in out.text.splitlines():
        line = raw.strip()
        if not line:
            continue
        try:
            event = _json.loads(line)
        except ValueError:
            continue  # a non-JSON line (e.g. a stderr warning merged in) — skip it
        if event.get("type") != "match":
            continue
        data = event["data"]
        path = data["path"].get("text", "")
        line_number = data.get("line_number")
        text = (data["lines"].get("text") or "").rstrip("\n")
        abs_offset = data.get("absolute_offset")
        for sm in data.get("submatches", []):
            rows.append(
                {
                    "path": path,
                    "line_number": line_number,
                    "col": sm.get("start"),
                    "match": (sm.get("match") or {}).get("text", ""),
                    "line": text,
                    "abs_offset": abs_offset,
                }
            )
            if len(rows) >= limit:
                return pl.DataFrame(rows, schema=_GREP_SCHEMA)
    return pl.DataFrame(rows, schema=_GREP_SCHEMA)


async def find(
    pattern: str = ".",
    root: str | os.PathLike[str] = ".",
    *,
    kind: str | None = None,
    ext: str | None = None,
    glob: bool = False,
    fixed: bool = False,
    hidden: bool = False,
    no_ignore: bool = False,
    max_depth: int | None = None,
    limit: int = DEFAULT_LIMIT,
    timeout: float = DEFAULT_TIMEOUT,
) -> pl.DataFrame:
    """Find files via fd, one row per path. ``pattern`` is a regex by default
    (``glob=True`` for glob, ``fixed=True`` for a literal); ``kind`` ∈
    file/dir/symlink; ``ext`` filters by extension. Respects ``.gitignore`` by
    default. Columns: ``path, name, type, size, mtime``."""
    argv = ["fd", "--print0"]
    if kind:
        argv += ["--type", _KIND_FLAG.get(kind, kind)]
    if ext:
        argv += ["--extension", ext]
    if glob:
        argv.append("--glob")
    if fixed:
        argv.append("--fixed-strings")
    if hidden:
        argv.append("--hidden")
    if no_ignore:
        argv.append("--no-ignore")
    if max_depth is not None:
        argv += ["--max-depth", str(max_depth)]
    argv += ["--", pattern, os.path.expanduser(str(root))]
    out = await _run(argv, timeout=timeout)
    paths = [p for p in out.text.split("\0") if p]
    return _lstat_rows(paths[:limit])


async def spotlight(
    query: str,
    root: str | os.PathLike[str] = ".",
    *,
    name_only: bool = False,
    literal: bool = False,
    limit: int = DEFAULT_LIMIT,
    timeout: float = DEFAULT_TIMEOUT,
) -> pl.DataFrame:
    """Full-text + metadata search via macOS Spotlight (mdfind), scoped to
    ``root``. ``name_only`` searches filenames; ``literal`` disables query
    interpretation. macOS only — raises FsearchError elsewhere. Columns:
    ``path, name, type, size, mtime``."""
    if sys.platform != "darwin":
        raise FsearchError("spotlight needs macOS Spotlight (mdfind); use grep/find on Linux")
    argv = ["/usr/bin/mdfind", "-0"]
    if root:
        argv += ["-onlyin", os.path.expanduser(str(root))]
    if name_only:
        argv.append("-name")
    if literal:
        argv.append("-literal")
    argv.append(query)
    out = await _run(argv, timeout=timeout)
    paths = [p for p in out.text.split("\0") if p]
    return _lstat_rows(paths[:limit])
