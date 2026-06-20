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

import asyncio
import base64
import json as _json
import os
import stat as _stat
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import polars as pl
from sh import Output, sh as _sh  # the bundled async shell-out helper; `sh.sh` is the function

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


def _expand(root: str | os.PathLike[str]) -> str:
    """Expand a leading ``~`` to an absolute path. Sync on purpose: ``expanduser``
    only reads ``$HOME`` / the passwd db (no event-loop I/O), and keeping it out
    of the ``async`` callers is what lets them stay free of path methods (ASYNC240)."""
    return str(Path(root).expanduser())


async def _run(argv: list[str], *, timeout: float, ok_codes: tuple[int, ...] = (0,)) -> Output:
    """Run a search CLI off the event loop with color disabled (so its output is
    clean, never SGR-corrupted). A non-success exit or a timeout surfaces as
    FsearchError, so callers have one error type to catch (the timeout is the
    safety net: a runaway search is killed at the deadline, never wedging the
    kernel)."""
    try:
        out = await _sh(argv, timeout=timeout, color=False)
    except TimeoutError as exc:
        raise FsearchError(f"{argv[0]} timed out after {timeout}s") from exc
    if out.code not in ok_codes:
        raise FsearchError(f"{argv[0]} exited {out.code}: {out.text.strip()[:500]}")
    return out


def _rg_str(field: dict[str, Any] | None) -> str:
    """Decode a ripgrep --json text field: it is `{"text": ...}` for UTF-8 and
    `{"bytes": "<base64>"}` for a non-UTF-8 path or match span. Reading only
    `text` would silently blank a hit in a non-UTF-8-named file."""
    if not field:
        return ""
    text = field.get("text")
    if text is not None:
        return text
    raw = field.get("bytes")
    if raw is not None:
        return base64.b64decode(raw).decode("utf-8", "surrogateescape")
    return ""


def _lstat_rows(paths: list[str]) -> pl.DataFrame:
    """Turn a list of NUL-separated paths into the find/spotlight frame, one
    os.lstat per path for type/size/mtime.

    fd/mdfind write paths to stdout, but `sh` merges stderr (mdfind's locale
    warnings, fd's permission-denied notes) into the same stream, so a stderr
    line can glue onto the first path. For each segment we take the real path
    (the text after any trailing stderr newline) and keep only segments that
    name an existing path — which drops the stderr noise without losing hits."""
    rows: list[dict[str, Any]] = []
    for raw in paths:
        if not raw:
            continue
        # Try the path verbatim first (preserves a legit leading/trailing space
        # or newline in a filename), then a stderr-stripped candidate.
        st = None
        cand = raw
        for attempt in (raw, raw.strip(), raw.rsplit("\n", 1)[-1].strip()):
            try:
                st = os.lstat(attempt)
                cand = attempt
                break
            except OSError:
                continue
        if st is None:
            continue  # not an existing path (a stderr line, or a vanished hit)
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
        rows.append(
            {
                "path": cand,
                "name": Path(cand.rstrip("/")).name,
                "type": kind,
                "size": st.st_size,
                "mtime": datetime.fromtimestamp(st.st_mtime, tz=UTC),
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
    argv += ["--", pattern, _expand(root)]
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
        path = _rg_str(data.get("path"))
        line_number = data.get("line_number")
        text = _rg_str(data.get("lines")).rstrip("\n")
        abs_offset = data.get("absolute_offset")
        for sm in data.get("submatches", []):
            rows.append(
                {
                    "path": path,
                    "line_number": line_number,
                    "col": sm.get("start"),
                    "match": _rg_str(sm.get("match")),
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
    argv += ["--max-results", str(limit)]  # cap at the source (limit applies to real hits)
    argv += ["--", pattern, _expand(root)]
    out = await _run(argv, timeout=timeout)
    paths = [p for p in out.text.split("\0") if p]
    # lstat off the event loop (up to `limit` stat syscalls), then cap rows.
    return (await asyncio.to_thread(_lstat_rows, paths)).head(limit)


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
        argv += ["-onlyin", _expand(root)]
    if name_only:
        argv.append("-name")
    if literal:
        argv.append("-literal")
    argv.append(query)
    out = await _run(argv, timeout=timeout)
    paths = [p for p in out.text.split("\0") if p]
    # mdfind has no result cap, so lstat all (off-loop) then cap real rows.
    return (await asyncio.to_thread(_lstat_rows, paths)).head(limit)
