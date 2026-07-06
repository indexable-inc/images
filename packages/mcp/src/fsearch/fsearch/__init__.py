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
import contextlib
import json as _json
import os
import signal
import stat as _stat
import sys
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import polars as pl
from sh import _exec as _sh  # the kernel-private process runner (public sh() is disabled; agents use nu)

__all__ = ["FsearchError", "PartialFrame", "find", "grep", "spotlight"]

__version__ = "0.1.0"

DEFAULT_LIMIT = 10_000
DEFAULT_TIMEOUT = 30.0
# StreamReader buffer for the rg --json line stream (one JSON event per line). A
# match on a very long line makes that event large; 64 MiB keeps a legitimate
# long-line hit parseable where the asyncio default (64 KiB) would raise.
_STREAM_LINE_LIMIT = 64 * 1024 * 1024

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


class PartialFrame(pl.DataFrame):
    """A search-result frame whose scan did not finish, so the rows are only the
    matches found before the cut-off (a timeout, or the ``limit`` cap).

    It IS a ``polars.DataFrame`` -- ``.filter``/``.sort``/``.head`` all work -- so
    a caller that ignores the truncation still gets usable rows. Two things flag
    the incompleteness so a caller cannot silently mistake a partial scan for a
    complete one: ``.truncated`` is ``True`` (a plain frame has no such
    attribute), and ``.reason`` explains the cut-off; the ``repr`` (what the
    dashboard/model see) leads with a one-line warning. A polars operation that
    builds a new frame returns a plain ``DataFrame`` -- the flag describes THIS
    scan, not a derived view, so it deliberately does not propagate."""

    # Class default so `getattr(frame, "truncated", False)` is safe on any frame
    # (a plain DataFrame returns the False default; a PartialFrame overrides it).
    truncated = True

    # `PartialFrame(existing_frame, ...)` relies on polars accepting a DataFrame
    # as the `data` argument (DataFrame-from-DataFrame init); the typecheckSmoke
    # nix test exercises this path, so a polars bump that drops it fails CI.
    def __init__(self, data: object = None, *args: object, reason: str = "", **kwargs: object) -> None:
        super().__init__(data, *args, **kwargs)
        self.reason = reason

    def __repr__(self) -> str:
        banner = f"[partial results: {self.reason}]\n" if self.reason else "[partial results]\n"
        return banner + super().__repr__()


def _expand(root: str | os.PathLike[str]) -> str:
    """Expand a leading ``~`` to an absolute path. Sync on purpose: ``expanduser``
    only reads ``$HOME`` / the passwd db (no event-loop I/O), and keeping it out
    of the ``async`` callers is what lets them stay free of path methods (ASYNC240)."""
    return str(Path(root).expanduser())


async def _run(argv: list[str], *, timeout: float, ok_codes: tuple[int, ...] = (0,)) -> tuple[str, bool]:
    """Run a search CLI off the event loop with color disabled (so its output is
    clean, never SGR-corrupted) and return ``(output_text, timed_out)``.

    On a timeout the search is killed at the deadline (the safety net: a runaway
    search never wedges the kernel), but its output-so-far is NOT discarded --
    ``sh`` attaches it to the ``TimeoutError`` as ``partial_output``, so this
    returns that text with ``timed_out=True`` and the caller parses the matches
    found before the cut-off. A non-success exit (other than the expected codes)
    still surfaces as FsearchError."""
    try:
        out = await _sh(argv, timeout=timeout, color=False)
    except TimeoutError as exc:
        # Recover whatever the child wrote before the deadline (see sh.sh); an
        # older sh without the attribute yields "" -- an empty partial, not a
        # crash.
        return (getattr(exc, "partial_output", "") or "", True)
    if out.code not in ok_codes:
        raise FsearchError(f"{argv[0]} exited {out.code}: {out.text.strip()[:500]}")
    return (out.text, False)


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
    rows, timed_out, hit_limit = await _stream_rg(argv, limit=limit, timeout=timeout)
    if timed_out:
        return PartialFrame(
            rows,
            schema=_GREP_SCHEMA,
            reason=f"rg timed out after {timeout}s; {len(rows)} match(es) found before the deadline",
        )
    if hit_limit:
        # The scan stopped at `limit` (rg was killed once enough matches were
        # parsed, rather than left to scan the whole tree): the rows are complete
        # up to the cap but the search did not exhaust `root`, so flag it.
        return PartialFrame(
            rows,
            schema=_GREP_SCHEMA,
            reason=f"stopped at limit={limit}; raise limit= to scan further",
        )
    return pl.DataFrame(rows, schema=_GREP_SCHEMA)


def _parse_rg_match(event: dict[str, Any]) -> list[dict[str, Any]]:
    """The submatch rows for one ripgrep ``--json`` ``match`` event (empty for a
    non-match event)."""
    if event.get("type") != "match":
        return []
    data = event["data"]
    path = _rg_str(data.get("path"))
    line_number = data.get("line_number")
    text = _rg_str(data.get("lines")).rstrip("\n")
    abs_offset = data.get("absolute_offset")
    return [
        {
            "path": path,
            "line_number": line_number,
            "col": sm.get("start"),
            "match": _rg_str(sm.get("match")),
            "line": text,
            "abs_offset": abs_offset,
        }
        for sm in data.get("submatches", [])
    ]


async def _stream_rg(
    argv: list[str], *, limit: int, timeout: float
) -> tuple[list[dict[str, Any]], bool, bool]:
    """Run ripgrep and parse its ``--json`` stream line by line, killing the
    process group as soon as ``limit`` match rows are collected.

    Returns ``(rows, timed_out, hit_limit)``. ripgrep has no global "stop after N
    total matches" flag (``--max-count`` is per file), so bounding a ``limit=``
    query means terminating rg once enough rows are parsed instead of letting it
    scan the whole tree and discarding the tail -- otherwise a broad pattern over
    a huge root pays the full-tree cost for a handful of wanted rows. The scan is
    still a separate process bounded by ``timeout`` (a process-group kill, like
    ``sh``), so a runaway never wedges the kernel's event loop.

    A backend failure is an error, not an empty result: rg exits 2 on a bad
    pattern, an unreadable root, or an invalid glob, and that raises
    :class:`FsearchError` carrying rg's own stderr -- an empty frame would be
    indistinguishable from a legitimate "no matches". A kill we initiated (the
    timeout, or the limit) is not a failure."""
    proc = await asyncio.create_subprocess_exec(
        *argv,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        start_new_session=True,  # own process group, so the kill reaps rg's children too
        # One rg --json event is a whole line; a match on a very long line (a
        # minified bundle, a data file) makes that line large, and the default
        # 64 KiB StreamReader buffer raises LimitOverrunError on it. Give the
        # buffer room so a legitimate long-line match parses instead of crashing
        # the search.
        limit=_STREAM_LINE_LIMIT,
    )
    rows: list[dict[str, Any]] = []
    hit_limit = False
    stderr_chunks: list[bytes] = []

    async def _drain_stderr() -> None:
        # Drained concurrently so a chatty rg can never fill the stderr pipe and
        # deadlock against the stdout reader; the text feeds the failure message.
        assert proc.stderr is not None
        while True:
            block = await proc.stderr.read(8192)
            if not block:
                break
            stderr_chunks.append(block)

    async def _read() -> None:
        nonlocal hit_limit
        assert proc.stdout is not None
        while True:
            try:
                raw = await proc.stdout.readline()
            except ValueError:
                # A single line still exceeded the (generous) buffer: skip past it
                # rather than abort the whole search. readline() consumed the
                # buffer, so the next read resumes after the oversized line.
                continue
            if not raw:
                break
            line = raw.decode("utf-8", "replace").strip()
            if not line:
                continue
            try:
                event = _json.loads(line)
            except ValueError:
                continue  # a non-JSON line (e.g. a stderr warning merged in) — skip it
            rows.extend(_parse_rg_match(event))
            if len(rows) >= limit:
                del rows[limit:]
                hit_limit = True
                return  # stop reading; the finally-block kills rg's whole group

    stderr_task = asyncio.ensure_future(_drain_stderr())
    timed_out = False
    try:
        await asyncio.wait_for(_read(), timeout)
    except TimeoutError:
        timed_out = True
    finally:
        _kill_group(proc)
        with contextlib.suppress(TimeoutError):
            await asyncio.wait_for(proc.wait(), 2.0)
        stderr_task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await stderr_task
    # rg's contract: 0 = matches, 1 = no matches, anything else = a real failure
    # (bad regex, unreadable root, invalid glob). Only a run WE cut short (the
    # deadline or the limit kill) is exempt.
    if not timed_out and not hit_limit and proc.returncode not in (0, 1):
        detail = b"".join(stderr_chunks).decode("utf-8", "replace").strip()
        raise FsearchError(f"{argv[0]} exited {proc.returncode}: {detail[:500]}")
    return (rows, timed_out, hit_limit)


def _kill_group(proc: asyncio.subprocess.Process) -> None:
    """SIGKILL the child's whole process group (best-effort). ``start_new_session``
    made the child a group leader, so one signal reaps rg and anything it spawned;
    a race where it already exited is ignored."""
    if proc.returncode is not None:
        return
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(proc.pid, signal.SIGKILL)


async def _paths_frame(text: str, *, limit: int, timed_out: bool, tool: str, timeout: float) -> pl.DataFrame:
    """The shared tail of ``find``/``spotlight``: parse the tool's NUL-separated
    paths, lstat them off the event loop into the find/spotlight frame, cap at
    ``limit``, and wrap it as a :class:`PartialFrame` when the scan timed out (so
    the paths found before the deadline are returned, flagged, not discarded)."""
    paths = [p for p in text.split("\0") if p]
    frame = (await asyncio.to_thread(_lstat_rows, paths)).head(limit)
    if timed_out:
        return PartialFrame(
            frame,
            reason=f"{tool} timed out after {timeout}s; {frame.height} path(s) found before the deadline",
        )
    return frame


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
    text, timed_out = await _run(argv, timeout=timeout)
    return await _paths_frame(text, limit=limit, timed_out=timed_out, tool="fd", timeout=timeout)


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
    # mdfind has no result cap, so _paths_frame lstats all (off-loop) then caps.
    text, timed_out = await _run(argv, timeout=timeout)
    return await _paths_frame(text, limit=limit, timed_out=timed_out, tool="mdfind", timeout=timeout)
