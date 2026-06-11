"""fff fast file-search, bound to Python through its stable C ABI.

Bundled into the pinned ix-mcp interpreter the same way `tui` and `search` are,
so every session can `import fff` with no install step. fff (dmtrKovalenko/fff)
is a typo-resistant fuzzy file finder and SIMD content grep with an in-memory
index; this module loads the `fff-c` cdylib (`packages/fff` emits it next to the
`fff-mcp` binary) via ctypes and exposes a small typed surface over it.

    import fff

    # one-shot helpers keep a cached, file-watching index per directory:
    res = fff.find(query="picker", path=".")
    for hit in res.hits:
        print(hit.path, hit.frecency)
    res[0]                              # results are subscriptable (and sliceable)
    Path(res[0]).read_text()           # a hit is os.PathLike -> its absolute path

    for m in fff.grep(query="fn main", path=".", mode="regex").matches:
        print(f"{m.path}:{m.line_number}: {m.line}")

    # one query, or many at once (matched as a literal OR in a single pass):
    fff.grep(query=["TODO", "FIXME", "XXX"], path=".", mode="plain")

    # or hold an instance for repeated queries against one tree:
    with fff.FileFinder(root=".", content_indexing=True) as ff:
        ff.wait_for_scan()              # block until the initial scan finishes
        ff.search(query="readme")      # fuzzy file search, frecency-ranked
        ff.glob(pattern="**/*.rs")     # literal glob, no fuzzy parsing
        ff.grep(query="TODO", mode="regex")  # content search (plain | regex | fuzzy)
        ff.multi_grep(patterns=["foo", "bar"])  # OR across many literals (Aho-Corasick)

Results are plain dataclasses (`FileHit`, `GrepMatch`, `SearchResult`,
`GrepResult`). Only the accessor-backed surfaces of fff-c are bound: file search
(`search`/`glob`) and content grep (`grep`/`multi_grep`). Directory and mixed
search are intentionally omitted because fff-c ships no stable field accessors
for them, and reading those `#[repr(C)]` structs by offset would break silently
on a layout change.

This is a thin FFI layer with no domain logic of its own: every behavior comes
from the one Rust core shared with fff-mcp and the Node/Neovim bindings. It runs
on Linux and macOS (wherever `packages/fff` builds the cdylib).
"""

from __future__ import annotations

import asyncio
import ctypes
import html
import json
import os
import shutil
import sys
import tempfile
import threading
from collections import OrderedDict
from dataclasses import dataclass, field, replace
from pathlib import Path

__all__ = [
    "FffError",
    "FileFinder",
    "FileHit",
    "GrepMatch",
    "GrepResult",
    "MatchRange",
    "SearchResult",
    "afind",
    "agrep",
    "find",
    "finder",
    "grep",
    "tree",
    "atree",
    "map",
    "amap",
    "CodeMap",
]

__version__ = "0.9.2"

# Mirrors `FFF_CREATE_OPTIONS_VERSION` in fff-c. The options struct only ever
# appends fields, so v1 stays valid forever; the library reads exactly the
# fields this version knows about.
_OPTIONS_VERSION = 1

# fff_live_grep mode byte: 0 plain (SIMD literal), 1 regex, 2 fuzzy.
_GREP_MODES = {"plain": 0, "literal": 0, "text": 0, "regex": 1, "fuzzy": 2}


class FffError(RuntimeError):
    """Raised when a fff-c call returns an error result."""


# ── library loading ────────────────────────────────────────────────────────


def _load_library() -> ctypes.CDLL:
    """Load the fff-c cdylib bundled next to this module.

    `packages/mcp` copies `libfff_c.so` (Linux) or `libfff_c.dylib` (macOS)
    into this package directory at build time, so the load is a fixed path with
    no `LD_LIBRARY_PATH`/`ctypes.util.find_library` search.
    """
    here = Path(__file__).resolve().parent
    names = ["libfff_c.dylib"] if sys.platform == "darwin" else ["libfff_c.so"]
    for name in names:
        candidate = here / name
        if candidate.exists():
            return ctypes.CDLL(str(candidate))
    # Fall back to any libfff_c.* so a platform mismatch surfaces clearly.
    for candidate in sorted(here.glob("libfff_c.*")):
        return ctypes.CDLL(str(candidate))
    raise ImportError(
        f"fff: no libfff_c cdylib found in {here}. The mcp package must bundle "
        "the fff-c shared library next to this module."
    )


_lib = _load_library()


# ── C structs we read directly (no stable accessor exists for these) ─────────


class _CreateOptions(ctypes.Structure):
    # Layout asserted stable by fff-c's options_layout_tests (88 bytes, 8-align).
    _fields_ = [
        ("version", ctypes.c_uint32),
        ("base_path", ctypes.c_char_p),
        ("frecency_db_path", ctypes.c_char_p),
        ("history_db_path", ctypes.c_char_p),
        ("enable_mmap_cache", ctypes.c_bool),
        ("enable_content_indexing", ctypes.c_bool),
        ("watch", ctypes.c_bool),
        ("ai_mode", ctypes.c_bool),
        ("log_file_path", ctypes.c_char_p),
        ("log_level", ctypes.c_char_p),
        ("cache_budget_max_files", ctypes.c_uint64),
        ("cache_budget_max_bytes", ctypes.c_uint64),
        ("cache_budget_max_file_size", ctypes.c_uint64),
        ("enable_fs_root_scanning", ctypes.c_bool),
        ("enable_home_dir_scanning", ctypes.c_bool),
    ]


class _Result(ctypes.Structure):
    # The envelope every fff_* call returns by pointer. `handle` carries the
    # typed payload (a result struct, an opaque instance, or a C string);
    # `int_value` carries simple scalar returns.
    _fields_ = [
        ("success", ctypes.c_bool),
        ("error", ctypes.c_char_p),
        ("handle", ctypes.c_void_p),
        ("int_value", ctypes.c_int64),
    ]


class _MatchRange(ctypes.Structure):
    # A highlight span within a matched grep line. fff-c exposes these only as a
    # raw array; the surrounding `FffGrepMatch` is read through accessors.
    _fields_ = [("start", ctypes.c_uint32), ("end", ctypes.c_uint32)]


# ── prototype binding ────────────────────────────────────────────────────────
#
# ctypes defaults every return type to C `int`, which truncates 64-bit pointers.
# Declare argtypes/restype for every symbol used so pointers round-trip intact.

_VP = ctypes.c_void_p
_CP = ctypes.c_char_p
_U8 = ctypes.c_uint8
_U32 = ctypes.c_uint32
_U64 = ctypes.c_uint64
_I64 = ctypes.c_int64
_I32 = ctypes.c_int32
_BOOL = ctypes.c_bool
_RESULT_P = ctypes.POINTER(_Result)


def _bind(name: str, restype, argtypes: list) -> None:
    fn = getattr(_lib, name)
    fn.restype = restype
    fn.argtypes = argtypes


_bind("fff_create_instance_with", _RESULT_P, [ctypes.POINTER(_CreateOptions)])
_bind("fff_destroy", None, [_VP])
_bind("fff_search", _RESULT_P, [_VP, _CP, _CP, _U32, _U32, _U32, _I32, _U32])
_bind("fff_glob", _RESULT_P, [_VP, _CP, _CP, _U32, _U32, _U32])
_bind(
    "fff_live_grep",
    _RESULT_P,
    [_VP, _CP, _U8, _U64, _U32, _BOOL, _U32, _U32, _U64, _U32, _U32, _BOOL],
)
_bind(
    "fff_multi_grep",
    _RESULT_P,
    [_VP, _CP, _CP, _U64, _U32, _BOOL, _U32, _U32, _U64, _U32, _U32, _BOOL],
)
_bind("fff_scan_files", _RESULT_P, [_VP])
_bind("fff_is_scanning", _BOOL, [_VP])
_bind("fff_wait_for_scan", _RESULT_P, [_VP, _U64])
_bind("fff_get_base_path", _RESULT_P, [_VP])
_bind("fff_health_check", _RESULT_P, [_VP, _CP])
_bind("fff_refresh_git_status", _RESULT_P, [_VP])

_bind("fff_free_result", None, [_RESULT_P])
_bind("fff_free_search_result", None, [_VP])
_bind("fff_free_grep_result", None, [_VP])
_bind("fff_free_string", None, [_VP])

# File-item + search-result accessors (stable named API; see fff-c accessors.rs).
_bind("fff_search_result_get_count", _U32, [_VP])
_bind("fff_search_result_get_total_matched", _U32, [_VP])
_bind("fff_search_result_get_total_files", _U32, [_VP])
_bind("fff_search_result_get_item", _VP, [_VP, _U32])
_bind("fff_file_item_get_relative_path", _CP, [_VP])
_bind("fff_file_item_get_file_name", _CP, [_VP])
_bind("fff_file_item_get_git_status", _CP, [_VP])
_bind("fff_file_item_get_size", _U64, [_VP])
_bind("fff_file_item_get_modified", _U64, [_VP])
_bind("fff_file_item_get_total_frecency_score", _I64, [_VP])
_bind("fff_file_item_get_is_binary", _BOOL, [_VP])

# Grep-match + grep-result accessors.
_bind("fff_grep_result_get_count", _U32, [_VP])
_bind("fff_grep_result_get_total_matched", _U32, [_VP])
_bind("fff_grep_result_get_total_files_searched", _U32, [_VP])
_bind("fff_grep_result_get_next_file_offset", _U32, [_VP])
_bind("fff_grep_result_get_match", _VP, [_VP, _U32])
_bind("fff_grep_match_get_relative_path", _CP, [_VP])
_bind("fff_grep_match_get_file_name", _CP, [_VP])
_bind("fff_grep_match_get_git_status", _CP, [_VP])
_bind("fff_grep_match_get_line_content", _CP, [_VP])
_bind("fff_grep_match_get_line_number", _U64, [_VP])
_bind("fff_grep_match_get_col", _U32, [_VP])
_bind("fff_grep_match_get_byte_offset", _U64, [_VP])
_bind("fff_grep_match_get_is_definition", _BOOL, [_VP])
_bind("fff_grep_match_get_is_binary", _BOOL, [_VP])
_bind("fff_grep_match_get_match_ranges_count", _U32, [_VP])
_bind("fff_grep_match_get_match_range", ctypes.POINTER(_MatchRange), [_VP, _U32])
_bind("fff_grep_match_get_context_before_count", _U32, [_VP])
_bind("fff_grep_match_get_context_before", _CP, [_VP, _U32])
_bind("fff_grep_match_get_context_after_count", _U32, [_VP])
_bind("fff_grep_match_get_context_after", _CP, [_VP, _U32])


# ── result envelope helpers ──────────────────────────────────────────────────


def _consume(result_ptr) -> tuple[int | None, int]:
    """Read and free a `*FffResult`, returning `(handle, int_value)`.

    Frees only the envelope (and its error string); the `handle` payload, when
    present, is still owned by the caller and freed with its typed free fn.
    """
    if not result_ptr:
        raise FffError("fff returned a null result")
    res = result_ptr.contents
    success = bool(res.success)
    error = res.error  # copied to a Python bytes here, before the free below
    handle = res.handle
    int_value = res.int_value
    _lib.fff_free_result(result_ptr)
    if not success:
        raise FffError(error.decode("utf-8", "replace") if error else "unknown fff error")
    return handle, int_value


def _str(value: bytes | None) -> str | None:
    return value.decode("utf-8", "replace") if value else None


def _encode(value) -> bytes | None:
    if value is None:
        return None
    return os.fspath(value).encode("utf-8")


def _u32(name: str, value: int) -> int:
    """Validate an unsigned-32 argument; ctypes would otherwise silently wrap."""
    # `bool` is an `int` subclass, so reject it explicitly: `limit=True` should
    # be a type error, not a silent 1.
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > 0xFFFFFFFF:
        raise ValueError(f"{name} must be an int in [0, 2**32); got {value!r}")
    return value


def _u64(name: str, value: int) -> int:
    """Validate an unsigned-64 argument; ctypes would otherwise silently wrap."""
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or value < 0
        or value > 0xFFFFFFFFFFFFFFFF
    ):
        raise ValueError(f"{name} must be an int in [0, 2**64); got {value!r}")
    return value


def _validate_grep_args(
    limit: int,
    max_matches_per_file: int,
    file_offset: int,
    before_context: int,
    after_context: int,
    max_file_size: int,
    time_budget_ms: int,
) -> None:
    _u32("limit", limit)
    _u32("max_matches_per_file", max_matches_per_file)
    _u32("file_offset", file_offset)
    _u32("before_context", before_context)
    _u32("after_context", after_context)
    _u64("max_file_size", max_file_size)
    _u64("time_budget_ms", time_budget_ms)


# ── result types ─────────────────────────────────────────────────────────────


def _polars():
    """The bundled ``polars`` module, or None. fff's core carries no polars
    dependency; the ``.df``/HTML views are a convenience for the ix-mcp kernel
    (where polars is always present), so import it lazily and degrade to text
    when it is absent."""
    try:
        import polars as pl

        return pl
    except Exception:
        return None


@dataclass(frozen=True)
class FileHit:
    """One file from `search`/`glob`, ranked by fuzzy score and frecency.

    ``path`` is relative to the index ``root`` (compact for display); ``abspath``
    joins the two. The hit is ``os.PathLike`` -- ``os.fspath(hit)`` returns the
    *absolute* path, so ``open(hit)`` / ``Path(hit).read_text()`` work from any
    cwd, not just the index root.
    """

    path: str
    name: str
    size: int
    modified: int
    frecency: int
    is_binary: bool
    git_status: str | None = None
    root: str | None = None

    @property
    def abspath(self) -> str:
        """The absolute filesystem path (``root`` joined with the relative
        ``path``), or ``path`` unchanged when no root is known."""
        return os.path.join(self.root, self.path) if self.root else self.path

    def __fspath__(self) -> str:
        return self.abspath


@dataclass(frozen=True)
class SearchResult:
    hits: list[FileHit]
    total_matched: int
    total_files: int

    def __iter__(self):
        return iter(self.hits)

    def __len__(self) -> int:
        return len(self.hits)

    def __getitem__(self, index):
        """Index a single ``FileHit`` (``result[0]``) or slice into a new
        ``SearchResult`` (``result[:10]``), so the result composes like a list."""
        if isinstance(index, slice):
            sliced = self.hits[index]
            return SearchResult(
                hits=sliced, total_matched=self.total_matched, total_files=self.total_files
            )
        return self.hits[index]

    @property
    def df(self):
        """The hits as a ``polars.DataFrame`` (composes with the polars API and
        renders as the dashboard's styled table). Requires polars."""
        pl = _polars()
        if pl is None:
            raise FffError("polars is not available; iterate .hits instead")
        return pl.DataFrame(
            [
                {
                    "path": h.path,
                    "name": h.name,
                    "size": h.size,
                    "modified": h.modified,
                    "frecency": h.frecency,
                    "git": h.git_status,
                    "binary": h.is_binary,
                }
                for h in self.hits
            ],
            schema={
                "path": pl.Utf8,
                "name": pl.Utf8,
                "size": pl.Int64,
                "modified": pl.Int64,
                "frecency": pl.Int64,
                "git": pl.Utf8,
                "binary": pl.Boolean,
            },
        ).with_columns(pl.from_epoch("modified", time_unit="s"))

    def _ix_to_frame_(self):
        """Kernel table protocol: render as this polars frame for both the human
        (styled HTML) and the model (compact CSV). None when polars is absent so
        the kernel falls back to the text repr."""
        return self.df if _polars() is not None else None

    def _repr_html_(self) -> str | None:
        """Render as the styled table for the dashboard (None when polars is
        absent, so the human falls back to the text repr)."""
        pl = _polars()
        return self.df._repr_html_() if pl is not None else None

    def __repr__(self) -> str:
        head = "\n".join(f"  {h.path}" for h in self.hits[:30])
        more = f"\n  ... ({len(self.hits) - 30} more)" if len(self.hits) > 30 else ""
        return (
            f"SearchResult: {len(self.hits)} of {self.total_files} files "
            f"(matched {self.total_matched})" + (f"\n{head}{more}" if self.hits else "")
        )


@dataclass(frozen=True)
class MatchRange:
    """Byte span `[start, end)` within a matched line, for highlighting."""

    start: int
    end: int


@dataclass(frozen=True)
class GrepMatch:
    path: str
    name: str
    line_number: int
    col: int
    byte_offset: int
    line: str
    is_definition: bool
    is_binary: bool
    git_status: str | None = None
    match_ranges: list[MatchRange] = field(default_factory=list)
    context_before: list[str] = field(default_factory=list)
    context_after: list[str] = field(default_factory=list)
    root: str | None = None

    @property
    def abspath(self) -> str:
        """The absolute path of the matched file (``root`` joined with the
        relative ``path``), or ``path`` unchanged when no root is known."""
        return os.path.join(self.root, self.path) if self.root else self.path

    def __fspath__(self) -> str:
        return self.abspath


@dataclass(frozen=True)
class GrepResult:
    matches: list[GrepMatch]
    total_matched: int
    total_files_searched: int
    next_file_offset: int

    def __iter__(self):
        return iter(self.matches)

    def __len__(self) -> int:
        return len(self.matches)

    def __getitem__(self, index):
        """Index a single ``GrepMatch`` (``result[0]``) or slice into a new
        ``GrepResult`` (``result[:10]``), so the result composes like a list."""
        if isinstance(index, slice):
            sliced = self.matches[index]
            return GrepResult(
                matches=sliced,
                total_matched=self.total_matched,
                total_files_searched=self.total_files_searched,
                next_file_offset=self.next_file_offset,
            )
        return self.matches[index]

    @property
    def df(self):
        """The matches as a ``polars.DataFrame`` (composes with the polars API
        and renders as the dashboard's styled table). Requires polars."""
        pl = _polars()
        if pl is None:
            raise FffError("polars is not available; iterate .matches instead")
        return pl.DataFrame(
            [
                {
                    "path": m.path,
                    "line": m.line_number,
                    "col": m.col,
                    "content": m.line,
                    "def": m.is_definition,
                    "git": m.git_status,
                }
                for m in self.matches
            ],
            schema={
                "path": pl.Utf8,
                "line": pl.Int64,
                "col": pl.Int64,
                "content": pl.Utf8,
                "def": pl.Boolean,
                "git": pl.Utf8,
            },
        )

    def _ix_to_frame_(self):
        """Kernel table protocol: render as this polars frame for both the human
        (styled HTML) and the model (compact CSV). None when polars is absent so
        the kernel falls back to the text repr."""
        return self.df if _polars() is not None else None

    def _repr_html_(self) -> str | None:
        """Render as the styled table for the dashboard (None when polars is
        absent, so the human falls back to the text repr)."""
        pl = _polars()
        return self.df._repr_html_() if pl is not None else None

    def __repr__(self) -> str:
        head = "\n".join(
            f"  {m.path}:{m.line_number}: {m.line.strip()}" for m in self.matches[:30]
        )
        more = f"\n  ... ({len(self.matches) - 30} more)" if len(self.matches) > 30 else ""
        return (
            f"GrepResult: {len(self.matches)} matches in "
            f"{self.total_files_searched} files (total {self.total_matched})"
            + (f"\n{head}{more}" if self.matches else "")
        )


# ── the finder ───────────────────────────────────────────────────────────────


class FileFinder:
    """An indexed view over one directory tree.

    Construction kicks off a background scan; call `wait_for_scan()` before the
    first query, or use the module-level `find`/`grep` helpers which do it for
    you. Close it with `close()` or a `with` block to release the native index.
    """

    def __init__(
        self,
        *,
        root,
        ai_mode: bool = True,
        watch: bool = False,
        content_indexing: bool = False,
        mmap_cache: bool = False,
        frecency_db=None,
        history_db=None,
    ) -> None:
        self._root = os.path.abspath(os.fspath(root))
        opts = _CreateOptions()
        opts.version = _OPTIONS_VERSION
        opts.base_path = _encode(self._root)
        opts.frecency_db_path = _encode(frecency_db)
        opts.history_db_path = _encode(history_db)
        opts.enable_mmap_cache = mmap_cache
        opts.enable_content_indexing = content_indexing
        opts.watch = watch
        opts.ai_mode = ai_mode
        handle, _ = _consume(_lib.fff_create_instance_with(ctypes.byref(opts)))
        self._handle = ctypes.c_void_p(handle)
        self._closed = False

    # -- lifecycle --

    def close(self) -> None:
        if not self._closed and self._handle:
            _lib.fff_destroy(self._handle)
            self._closed = True

    def __enter__(self) -> "FileFinder":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass

    def _check_open(self) -> None:
        if self._closed:
            raise FffError("FileFinder is closed")

    # -- indexing --

    def scan(self) -> None:
        """Trigger a full rescan in the background (returns immediately)."""
        self._check_open()
        _consume(_lib.fff_scan_files(self._handle))

    def is_scanning(self) -> bool:
        self._check_open()
        return bool(_lib.fff_is_scanning(self._handle))

    def wait_for_scan(self, timeout_ms: int = 5000) -> bool:
        """Block until the initial scan finishes; True if it completed in time."""
        self._check_open()
        _u64("timeout_ms", timeout_ms)
        _, completed = _consume(_lib.fff_wait_for_scan(self._handle, timeout_ms))
        return bool(completed)

    def refresh_git_status(self) -> int:
        """Refresh the git-status cache; returns the number of files updated."""
        self._check_open()
        _, count = _consume(_lib.fff_refresh_git_status(self._handle))
        return count

    @property
    def base_path(self) -> str:
        self._check_open()
        handle, _ = _consume(_lib.fff_get_base_path(self._handle))
        try:
            return _str(ctypes.cast(ctypes.c_void_p(handle), _CP).value) or ""
        finally:
            _lib.fff_free_string(ctypes.c_void_p(handle))

    def health_check(self, test_path=None) -> dict:
        self._check_open()
        handle, _ = _consume(_lib.fff_health_check(self._handle, _encode(test_path)))
        try:
            raw = ctypes.cast(ctypes.c_void_p(handle), _CP).value
            return json.loads(raw.decode("utf-8")) if raw else {}
        finally:
            _lib.fff_free_string(ctypes.c_void_p(handle))

    # -- file search --

    def search(
        self,
        *,
        query: str,
        limit: int = 100,
        page: int = 0,
        current_file=None,
        max_threads: int = 0,
    ) -> SearchResult:
        """Fuzzy file search, ranked by match score combined with frecency."""
        self._check_open()
        _u32("limit", limit)
        _u32("page", page)
        _u32("max_threads", max_threads)
        handle, _ = _consume(
            _lib.fff_search(
                self._handle,
                query.encode("utf-8"),
                _encode(current_file),
                max_threads,
                page,
                limit,
                0,
                0,
            )
        )
        try:
            return self._read_search(handle)
        finally:
            _lib.fff_free_search_result(ctypes.c_void_p(handle))

    def glob(
        self,
        *,
        pattern: str,
        limit: int = 100,
        page: int = 0,
        current_file=None,
        max_threads: int = 0,
    ) -> SearchResult:
        """Literal glob filter (e.g. `*.rs`, `src/**`), ranked by frecency."""
        self._check_open()
        _u32("limit", limit)
        _u32("page", page)
        _u32("max_threads", max_threads)
        handle, _ = _consume(
            _lib.fff_glob(
                self._handle,
                pattern.encode("utf-8"),
                _encode(current_file),
                max_threads,
                page,
                limit,
            )
        )
        try:
            return self._read_search(handle)
        finally:
            _lib.fff_free_search_result(ctypes.c_void_p(handle))

    def _read_search(self, handle: int | None) -> SearchResult:
        h = ctypes.c_void_p(handle)
        count = _lib.fff_search_result_get_count(h)
        items = []
        for i in range(count):
            it = _lib.fff_search_result_get_item(h, i)
            items.append(
                FileHit(
                    path=_str(_lib.fff_file_item_get_relative_path(it)) or "",
                    name=_str(_lib.fff_file_item_get_file_name(it)) or "",
                    size=_lib.fff_file_item_get_size(it),
                    modified=_lib.fff_file_item_get_modified(it),
                    frecency=_lib.fff_file_item_get_total_frecency_score(it),
                    is_binary=bool(_lib.fff_file_item_get_is_binary(it)),
                    git_status=_str(_lib.fff_file_item_get_git_status(it)),
                    root=self._root,
                )
            )
        return SearchResult(
            hits=items,
            total_matched=_lib.fff_search_result_get_total_matched(h),
            total_files=_lib.fff_search_result_get_total_files(h),
        )

    # -- content grep --

    def grep(
        self,
        *,
        query: str,
        mode: str = "plain",
        limit: int = 50,
        max_matches_per_file: int = 0,
        smart_case: bool = True,
        file_offset: int = 0,
        max_file_size: int = 0,
        time_budget_ms: int = 0,
        before_context: int = 0,
        after_context: int = 0,
        classify_definitions: bool = False,
    ) -> GrepResult:
        """Content search across indexed files.

        ``mode`` defaults to ``"plain"`` (fast SIMD literal; aliases
        ``"literal"`` and ``"text"``); pass ``"regex"`` or ``"fuzzy"`` when
        needed.
        """
        self._check_open()
        if mode not in _GREP_MODES:
            raise ValueError(
                f"unknown grep mode {mode!r}; use 'plain' (aliases 'literal'/'text'), 'regex', or 'fuzzy'"
            )
        mode_byte = _GREP_MODES[mode]
        _validate_grep_args(
            limit,
            max_matches_per_file,
            file_offset,
            before_context,
            after_context,
            max_file_size,
            time_budget_ms,
        )
        handle, _ = _consume(
            _lib.fff_live_grep(
                self._handle,
                query.encode("utf-8"),
                mode_byte,
                max_file_size,
                max_matches_per_file,
                smart_case,
                file_offset,
                limit,
                time_budget_ms,
                before_context,
                after_context,
                classify_definitions,
            )
        )
        try:
            return self._read_grep(handle)
        finally:
            _lib.fff_free_grep_result(ctypes.c_void_p(handle))

    def multi_grep(
        self,
        *,
        patterns: list[str],
        constraints: str | None = None,
        limit: int = 50,
        max_matches_per_file: int = 0,
        smart_case: bool = True,
        file_offset: int = 0,
        max_file_size: int = 0,
        time_budget_ms: int = 0,
        before_context: int = 0,
        after_context: int = 0,
        classify_definitions: bool = False,
    ) -> GrepResult:
        """Match any of several literal patterns (Aho-Corasick), one pass.

        `constraints` is an optional file filter like `"*.rs"` or `"/src/"`.
        """
        self._check_open()
        if not patterns or all(not p for p in patterns):
            raise ValueError("multi_grep requires at least one non-empty pattern")
        if any("\n" in p for p in patterns):
            raise ValueError("multi_grep patterns must not contain newlines")
        _validate_grep_args(
            limit,
            max_matches_per_file,
            file_offset,
            before_context,
            after_context,
            max_file_size,
            time_budget_ms,
        )
        handle, _ = _consume(
            _lib.fff_multi_grep(
                self._handle,
                "\n".join(patterns).encode("utf-8"),
                _encode(constraints),
                max_file_size,
                max_matches_per_file,
                smart_case,
                file_offset,
                limit,
                time_budget_ms,
                before_context,
                after_context,
                classify_definitions,
            )
        )
        try:
            return self._read_grep(handle)
        finally:
            _lib.fff_free_grep_result(ctypes.c_void_p(handle))

    # -- async wrappers --
    #
    # fff's heavy work runs in the Rust core behind a ctypes call, which releases
    # the GIL for its duration (like numpy). Running a query through
    # ``asyncio.to_thread`` therefore runs it off the event loop, concurrently
    # with other async jobs, without blocking them. Same arguments as the sync
    # methods; ``await ff.grep_async("TODO")``.

    async def search_async(self, *args, **kwargs) -> SearchResult:
        return await asyncio.to_thread(self.search, *args, **kwargs)

    async def glob_async(self, *args, **kwargs) -> SearchResult:
        return await asyncio.to_thread(self.glob, *args, **kwargs)

    async def grep_async(self, *args, **kwargs) -> GrepResult:
        return await asyncio.to_thread(self.grep, *args, **kwargs)

    async def multi_grep_async(self, *args, **kwargs) -> GrepResult:
        return await asyncio.to_thread(self.multi_grep, *args, **kwargs)

    async def wait_for_scan_async(self, *args, **kwargs) -> bool:
        return await asyncio.to_thread(self.wait_for_scan, *args, **kwargs)

    def _read_grep(self, handle: int | None) -> GrepResult:
        h = ctypes.c_void_p(handle)
        count = _lib.fff_grep_result_get_count(h)
        matches = []
        for i in range(count):
            m = _lib.fff_grep_result_get_match(h, i)
            ranges = [
                MatchRange(start=r.contents.start, end=r.contents.end)
                for j in range(_lib.fff_grep_match_get_match_ranges_count(m))
                if (r := _lib.fff_grep_match_get_match_range(m, j))
            ]
            before = [
                _str(_lib.fff_grep_match_get_context_before(m, j)) or ""
                for j in range(_lib.fff_grep_match_get_context_before_count(m))
            ]
            after = [
                _str(_lib.fff_grep_match_get_context_after(m, j)) or ""
                for j in range(_lib.fff_grep_match_get_context_after_count(m))
            ]
            matches.append(
                GrepMatch(
                    path=_str(_lib.fff_grep_match_get_relative_path(m)) or "",
                    name=_str(_lib.fff_grep_match_get_file_name(m)) or "",
                    line_number=_lib.fff_grep_match_get_line_number(m),
                    col=_lib.fff_grep_match_get_col(m),
                    byte_offset=_lib.fff_grep_match_get_byte_offset(m),
                    line=_str(_lib.fff_grep_match_get_line_content(m)) or "",
                    is_definition=bool(_lib.fff_grep_match_get_is_definition(m)),
                    is_binary=bool(_lib.fff_grep_match_get_is_binary(m)),
                    git_status=_str(_lib.fff_grep_match_get_git_status(m)),
                    match_ranges=ranges,
                    context_before=before,
                    context_after=after,
                    root=self._root,
                )
            )
        return GrepResult(
            matches=matches,
            total_matched=_lib.fff_grep_result_get_total_matched(h),
            total_files_searched=_lib.fff_grep_result_get_total_files_searched(h),
            next_file_offset=_lib.fff_grep_result_get_next_file_offset(h),
        )


# ── module-level convenience over a cached, watched index per directory ──────
#
# One finder per directory, shared by `find` and `grep`. It is content-indexed
# and watching, so repeated queries against the same tree skip the rescan cost
# and `grep` always gets the SIMD content index. The cache is a bounded LRU, so
# the live native instances and watcher threads stay bounded by `_CACHE_MAX`.

_CACHE_MAX = 8
_cache: OrderedDict[str, FileFinder] = OrderedDict()
_cache_lock = threading.Lock()


def finder(*, root, **kwargs) -> FileFinder:
    """Construct a `FileFinder` (does not scan; call `wait_for_scan` yourself)."""
    return FileFinder(root=root, **kwargs)


def _cached(root) -> FileFinder:
    key = os.path.abspath(os.fspath(root))
    with _cache_lock:
        existing = _cache.get(key)
        if existing is not None and not existing._closed:
            _cache.move_to_end(key)
            return existing
        ff = FileFinder(root=key, watch=True, content_indexing=True)
        ff.wait_for_scan(10_000)
        _cache[key] = ff  # a fresh insert is already the most-recent (LRU) end
        while len(_cache) > _CACHE_MAX:
            # Drop the LRU entry but do NOT close() it here. A concurrent caller
            # may still hold this finder and be mid-query on it (ctypes releases
            # the GIL across the native call), so destroying the handle now would
            # be a use-after-free. Removing the cache's reference is enough:
            # FileFinder.__del__ reclaims the native instance once the last
            # caller reference is gone, which is the only safe point to destroy
            # it. Callers only ever hold a finder transiently (find/grep return
            # results, not the finder), so this reclaims promptly under CPython
            # refcounting and the watcher count still stays bounded.
            _cache.popitem(last=False)
        return ff


def _split_path(path) -> tuple[str, str | None]:
    """Split a search `path` into a directory root and an optional single-file
    name.

    A leading ``~`` is expanded, so ``"~/.zshrc"`` works the same as the
    explicitly expanded path. When `path` is a file, return its parent directory
    and basename; the single-file case is then served by :func:`_isolated_file_finder`
    (fff-c can't content-index a lone file as a root, and its indexer skips
    dotfiles and ignored paths). A directory passes through unscoped.
    """
    fspath = os.path.expanduser(os.fspath(path))
    if os.path.isfile(fspath):
        absolute = os.path.abspath(fspath)
        return os.path.dirname(absolute), os.path.basename(absolute)
    return fspath, None


def _is_unindexable_root(directory: str) -> bool:
    """True for directories fff-c refuses to index as a base: the filesystem
    root and the user's home directory.

    Indexing either walks an enormous tree and floods the file watcher, so fff-c
    rejects them up front (mirrors the `dirs::home_dir()` / `parent()` check in
    fff-core's ``FilePicker::new``).
    """
    resolved = os.path.abspath(directory)
    if os.path.dirname(resolved) == resolved:  # filesystem root ("/", "C:\\")
        return True
    home = os.path.abspath(os.path.expanduser("~"))
    return os.path.normcase(resolved) == os.path.normcase(home)


def _refuse_unindexable_dir(root: str) -> "FffError":
    """A clear, actionable error for a bare home / fs-root *directory*.

    Indexing the whole of ``$HOME`` or ``/`` is the one case fff genuinely won't
    serve. Pre-empt fff-c's terse "consider smaller per-project directories"
    (whose suggestion nudges toward shelling out to ``grep``) with guidance that
    keeps you on the fast in-process tool: name a file or a scoped subdir.
    """
    resolved = os.path.abspath(root)
    what = "filesystem root" if os.path.dirname(resolved) == resolved else "home directory"
    return FffError(
        f"refusing to index the {what} ({resolved}) whole: it would walk a huge "
        "tree and flood the file watcher. Search a specific file (e.g. "
        "'~/.zshrc') or a scoped subdirectory (e.g. '~/.config/nvim') instead."
    )


def _isolated_file_finder(abspath: str, tmp: str, *, content_indexing: bool) -> "FileFinder":
    """A finder over a throwaway directory holding only a copy of ``abspath``.

    An explicitly named file can't always be searched in place: fff-c can't
    content-index a lone file as its root, its indexer skips dotfiles and
    ignored paths, and the file may sit under a ``$HOME``/``/`` that fff won't
    index at all. So copy it into ``tmp`` under a *visible* name (leading dots
    stripped — the indexer ignores hidden files) and index that one-file tree:
    fff's real engine then runs over it (every grep mode, match ranges,
    definitions, binary detection). ``copy2`` preserves size and mtime. Results
    come back keyed by the copy's name, so callers remap them to the real one.
    """
    copy_name = os.path.basename(abspath).lstrip(".") or "file"
    shutil.copy2(abspath, os.path.join(tmp, copy_name))
    ff = FileFinder(root=tmp, watch=False, content_indexing=content_indexing)
    ff.wait_for_scan(10_000)
    return ff


def find(query: str, path=".", *, limit: int = 100) -> SearchResult:
    """Fuzzy file search over `path=` (root directory or file), reusing a cached watched index.

    `path` may be a directory (searched whole) or a single file (searched on its
    own, in an isolated one-file index so even a dotfile or a file directly under
    `$HOME`/`/` is matched). A bare `~`/`/` *directory* is refused with guidance
    toward a file or a scoped subdirectory.
    """
    root, only = _split_path(path)
    if only is None:
        if _is_unindexable_root(root):
            raise _refuse_unindexable_dir(root)
        return _cached(root).search(query=query, limit=limit)
    with tempfile.TemporaryDirectory(prefix="ix-fff-file-") as tmp:
        with _isolated_file_finder(os.path.join(root, only), tmp, content_indexing=False) as ff:
            result = ff.search(query=query, limit=limit)
    hits = [replace(h, path=only, name=only, root=root) for h in result.hits][:limit]
    return SearchResult(hits=hits, total_matched=len(hits), total_files=len(hits))


def tree(path=".", *, glob: str = "**", limit: int = 10_000) -> SearchResult:
    """The file tree under `path=` as data: every (gitignore-aware) file with its
    size and mtime, reusing a cached watched index.

    This is the primitive for "what is in this directory?" -- reach for it
    instead of hand-rolling `os.walk` with ad-hoc ignore lists. The result's
    ``.df`` is a polars frame (``path``, ``name``, ``size``, ``modified``, ...),
    so shaping is a polars expression, not a loop::

        fff.tree("packages/mcp").df.sort("size", descending=True).head(20)
        fff.tree(".").df.filter(pl.col("path").str.contains("site/"))

    ``glob`` narrows the listing natively (e.g. ``"src/**"``, ``"*.rs"``); the
    default ``"**"`` lists everything. Honors the same ignore rules as `find`/
    `grep` (gitignore, hidden files), and refuses a bare `~`/`/` root with
    guidance toward a scoped subdirectory.
    """
    root, only = _split_path(path)
    if only is not None:
        # A single file is a one-row tree; stat it directly rather than spinning
        # up an isolated index.
        stat = os.stat(os.path.join(root, only))
        hit = FileHit(
            path=only,
            name=only,
            size=stat.st_size,
            modified=int(stat.st_mtime),
            frecency=0,
            is_binary=False,
            root=root,
        )
        return SearchResult(hits=[hit], total_matched=1, total_files=1)
    if _is_unindexable_root(root):
        raise _refuse_unindexable_dir(root)
    return _cached(root).glob(pattern=glob, limit=limit)


async def atree(path=".", *, glob: str = "**", limit: int = 10_000) -> SearchResult:
    """Async file-tree listing: runs off the event loop (non-blocking)."""
    return await asyncio.to_thread(tree, path=path, glob=glob, limit=limit)


def grep(query: str | list[str], path=".", *, mode: str = "plain", limit: int = 50, glob: str | None = None) -> GrepResult:
    """Content grep over `path=` (root directory or file), reusing a cached watched (content-indexed) index.

    `query` and `path` are positional like the shell's `grep PATTERN PATH`
    (path defaults to the current directory); the options stay keyword-only.
    `query` is one pattern, or a list of patterns matched as literals in a single
    OR pass (Aho-Corasick) -- the one call for "where does any of these appear?",
    so you never loop grep over a list. `mode` defaults to ``"plain"``; pass
    ``"regex"`` or ``"fuzzy"`` when needed:
      - one string: ``"plain"`` (fast SIMD literal; aliases ``"literal"`` and
        ``"text"``), ``"regex"``, or ``"fuzzy"``;
      - a list: matched literally, so pass ``"plain"`` or ``"literal"`` (for a
        regex, pass one string like ``"a|b"`` with ``"regex"``).

    `glob` is an optional file-name filter applied before matching, e.g.
    ``glob="*.rs"`` to restrict a content search to Rust files in a mixed-language
    repo. For a list query the glob is applied at the native level (inside the
    Aho-Corasick pass, so only matching-extension files are searched and the
    ``limit`` is not wasted on excluded files). For a single-pattern query the glob
    is applied as a post-filter on the returned matches, since the native single-
    pattern engine has no per-file selector; in that case `limit` applies before the
    glob, so very tight limits may appear to return fewer results than expected --
    raise `limit` if you see this.

    `path` may be a directory (grepped whole) or a single file. A single file is
    grepped in an isolated one-file index, so it works even for a dotfile (which
    the indexer skips in place) or a file directly under `$HOME`/`/` (which fff
    won't index whole). A bare `~`/`/` *directory* is refused with guidance
    toward a file or a scoped subdirectory.
    """
    import fnmatch as _fnmatch

    multi = not isinstance(query, str)
    if multi and _GREP_MODES.get(mode) != 0:
        raise ValueError(
            'a list of patterns is matched literally (OR across them); pass '
            'mode="plain" (or alias "literal"/"text"). For a regex, pass a '
            'single pattern string with mode="regex" (e.g. "a|b").'
        )

    def _run(ff: "FileFinder") -> GrepResult:
        if multi:
            return ff.multi_grep(patterns=query, constraints=glob, limit=limit)
        result = ff.grep(query=query, mode=mode, limit=limit)
        if glob is None:
            return result
        # Post-filter by glob for single-pattern mode (no native file selector).
        filtered = [m for m in result.matches if _fnmatch.fnmatch(m.name, glob)]
        return GrepResult(
            matches=filtered,
            total_matched=len(filtered),
            total_files_searched=result.total_files_searched,
            next_file_offset=result.next_file_offset,
        )

    root, only = _split_path(path)
    if only is None:
        if _is_unindexable_root(root):
            raise _refuse_unindexable_dir(root)
        return _run(_cached(root))
    with tempfile.TemporaryDirectory(prefix="ix-fff-file-") as tmp:
        with _isolated_file_finder(os.path.join(root, only), tmp, content_indexing=True) as ff:
            result = _run(ff)
    matches = [replace(m, path=only, name=only, root=root) for m in result.matches]
    return GrepResult(
        matches=matches,
        total_matched=result.total_matched,
        total_files_searched=result.total_files_searched,
        next_file_offset=0,
    )


async def afind(query: str, path=".", *, limit: int = 100) -> SearchResult:
    """Async fuzzy file search: runs off the event loop (non-blocking)."""
    return await asyncio.to_thread(find, query=query, path=path, limit=limit)


async def agrep(query: str | list[str], path=".", *, mode: str = "plain", limit: int = 50, glob: str | None = None) -> GrepResult:
    """Async content grep: runs off the event loop (non-blocking)."""
    return await asyncio.to_thread(grep, query=query, path=path, mode=mode, limit=limit, glob=glob)


class CodeMap:
    """A grep result grouped into a glanceable code map: hits per file, with
    definitions (\u25cf) ranked above references (\u25cb).

    ``repr`` is a compact text tree for the agent; ``_repr_html_`` is a render
    where each file is a native ``<details>`` you can fold, so a wide search
    stays scannable on the dashboard.
    """

    def __init__(self, *, query: "str | list[str]", matches: list["GrepMatch"]) -> None:
        self.query = query
        # A list query (multi-pattern OR) displays as ``a | b`` in the headers.
        self._query_str = query if isinstance(query, str) else " | ".join(query)
        self.matches = matches
        self.by_file: dict[str, list["GrepMatch"]] = {}
        for m in matches:
            self.by_file.setdefault(m.path, []).append(m)
        for hits in self.by_file.values():
            hits.sort(key=lambda h: (not h.is_definition, h.line_number))

    @property
    def defs(self) -> list["GrepMatch"]:
        return [m for m in self.matches if m.is_definition]

    @property
    def df(self):
        """The map as a nested ``polars.DataFrame``: one row per file, with the
        per-file matches as a ``list[struct]`` of (line, col, def, content), and
        ``defs``/``hits`` counts. Requires polars. (For a flat table use
        ``fff.grep(...).df``; the nested frame is for grouping/aggregation.)"""
        pl = _polars()
        if pl is None:
            raise FffError("polars is not available; iterate .by_file instead")
        rows = [
            {
                "path": path,
                "defs": sum(1 for h in hits if h.is_definition),
                "hits": len(hits),
                "matches": [
                    {
                        "line": h.line_number,
                        "col": h.col,
                        "def": h.is_definition,
                        "content": h.line.strip(),
                    }
                    for h in hits
                ],
            }
            for path, hits in self.by_file.items()
        ]
        return pl.DataFrame(
            rows,
            schema={
                "path": pl.Utf8,
                "defs": pl.Int64,
                "hits": pl.Int64,
                "matches": pl.List(
                    pl.Struct(
                        {
                            "line": pl.Int64,
                            "col": pl.Int64,
                            "def": pl.Boolean,
                            "content": pl.Utf8,
                        }
                    )
                ),
            },
        )

    def __repr__(self) -> str:
        if not self.matches:
            return f"no matches for {self._query_str!r}"
        lines = [
            f"{self._query_str}  ({len(self.defs)} def, {len(self.matches)} hits, "
            f"{len(self.by_file)} files)"
        ]
        for path, hits in self.by_file.items():
            lines.append(f" {path}")
            for m in hits:
                mark = "\u25cf" if m.is_definition else "\u25cb"
                lines.append(f"   {mark} {m.line_number:>5}  {m.line.strip()}")
        return "\n".join(lines)

    def _repr_html_(self) -> str:
        if not self.matches:
            return (
                '<div style="color:#6a6a70;font-style:italic">'
                f"no matches for {html.escape(self._query_str)}</div>"
            )
        mono = "ui-monospace,SFMono-Regular,Menlo,monospace"
        blocks = []
        for path, hits in self.by_file.items():
            ndef = sum(1 for h in hits if h.is_definition)
            rows = []
            for m in hits:
                mark = "\u25cf" if m.is_definition else "\u25cb"
                mark_col = "#e6e6e6" if m.is_definition else "#55555b"
                line_html = html.escape(m.line.rstrip())
                rows.append(
                    '<div style="display:flex;gap:10px;padding:1px 0">'
                    f'<span style="color:{mark_col};width:1em">{mark}</span>'
                    f'<span style="color:#6a6a70;min-width:3.5em;text-align:right">{m.line_number}</span>'
                    f'<span style="color:#bcbcc2;white-space:pre;overflow:hidden;text-overflow:ellipsis">{line_html}</span>'
                    "</div>"
                )
            summary = (
                '<summary style="cursor:pointer;color:#e6e6e6;padding:4px 0">'
                f"{html.escape(path)} "
                f'<span style="color:#6a6a70">\u00b7 {ndef} def / {len(hits)} hits</span>'
                "</summary>"
            )
            blocks.append(
                '<details open style="border-top:1px solid #242427;padding:4px 10px">'
                f"{summary}<div>{''.join(rows)}</div></details>"
            )
        head = (
            '<div style="color:#6a6a70;padding:6px 10px">'
            f"{html.escape(self._query_str)} \u00b7 {len(self.defs)} def \u00b7 "
            f"{len(self.matches)} hits \u00b7 {len(self.by_file)} files</div>"
        )
        return (
            '<div style="background:#141416;border:1px solid #242427;border-radius:6px;'
            f'color:#e6e6e6;font-family:{mono};font-size:12px;overflow:auto">'
            f"{head}{''.join(blocks)}</div>"
        )


def map(*, query: str | list[str], path, mode: str = "plain", limit: int = 200, glob: str | None = None) -> CodeMap:
    """Content grep grouped into a :class:`CodeMap`: hits per file with
    definitions ranked first. A glanceable answer to "where is X defined and
    used?" built straight on :func:`grep`. Accepts the same ``glob`` filter as
    :func:`grep` to scope results to files matching a pattern (e.g. ``"*.rs"``)."""
    return CodeMap(query=query, matches=grep(query=query, path=path, mode=mode, limit=limit, glob=glob).matches)


async def amap(*, query: str | list[str], path, mode: str = "plain", limit: int = 200, glob: str | None = None) -> CodeMap:
    """Async :func:`map`: the same code map, off the event loop."""
    res = await agrep(query=query, path=path, mode=mode, limit=limit, glob=glob)
    return CodeMap(query=query, matches=res.matches)
