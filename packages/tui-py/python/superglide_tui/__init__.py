"""High-level Python API for the `tui` PTY-backed terminal manager.

Spawn child processes attached to real pseudo-terminals, drive them with
keystrokes, and observe their VT100-rendered viewport. The public surface is:

    TuiManager      spawn and track many concurrent processes
    Tui             a single spawned process; the workhorse handle
    Snapshot        an immutable read-time view of one process
    Size            (rows, cols) terminal size
    Key             common keystrokes as ANSI byte strings, with .ctrl/.alt
    StyledCell      one viewport cell: character + VT100 attributes
    WaitTimeout     raised by `Tui.wait_for(...)` when nothing matched in time

Every blocking I/O method has an `a`-prefixed coroutine variant that returns a
native asyncio-awaitable from the Rust side (via pyo3-async-runtimes); no
thread-pool hop is involved.
"""

from __future__ import annotations

import asyncio
import re
import time
import uuid
from collections.abc import Callable, Iterable, Iterator
from dataclasses import dataclass
from enum import StrEnum
from types import TracebackType
from typing import Self, TypeAlias

import numpy as np
from numpy.typing import NDArray

from ._superglide_tui import (
    TuiInstance as _RawTuiInstance,
    TuiManager as _RawTuiManager,
    __version__,
)

__all__ = [
    "Key",
    "Pattern",
    "Size",
    "Snapshot",
    "StyledCell",
    "Tui",
    "TuiManager",
    "WaitTimeout",
    "__version__",
]


# --------------------------------------------------------------------------- #
# Value types
# --------------------------------------------------------------------------- #


@dataclass(frozen=True, slots=True)
class Size:
    """Terminal dimensions, in cells."""

    rows: int
    cols: int

    def __iter__(self) -> Iterator[int]:
        yield self.rows
        yield self.cols


@dataclass(frozen=True, slots=True)
class StyledCell:
    """One viewport cell with its VT100 attributes.

    `fg`/`bg` are vt100-formatted color strings or `None` for the default.
    """

    char: str
    fg: str | None
    bg: str | None
    bold: bool
    italic: bool
    underline: bool
    inverse: bool


@dataclass(frozen=True, slots=True)
class Snapshot:
    """An immutable view of a Tui at a single point in time."""

    viewport: tuple[str, ...]
    scrollback: tuple[str, ...]
    size: Size

    @property
    def text(self) -> str:
        """Viewport joined with newlines."""
        return "\n".join(self.viewport)

    @property
    def full_text(self) -> str:
        """Scrollback + viewport joined with newlines."""
        return "\n".join((*self.scrollback, *self.viewport))

    def __contains__(self, needle: object) -> bool:
        return isinstance(needle, str) and needle in self.text

    def __str__(self) -> str:
        return self.text


# --------------------------------------------------------------------------- #
# Keystrokes
# --------------------------------------------------------------------------- #


class Key(StrEnum):
    """Common keystrokes as ANSI byte sequences.

    Members are plain strings, so they concatenate with literal text and pass
    straight to `Tui.send(...)`.
    """

    ENTER = "\r"
    TAB = "\t"
    BACKTAB = "\x1b[Z"
    ESC = "\x1b"
    BACKSPACE = "\x7f"
    DELETE = "\x1b[3~"
    SPACE = " "

    UP = "\x1b[A"
    DOWN = "\x1b[B"
    RIGHT = "\x1b[C"
    LEFT = "\x1b[D"

    HOME = "\x1b[H"
    END = "\x1b[F"
    PAGE_UP = "\x1b[5~"
    PAGE_DOWN = "\x1b[6~"

    F1 = "\x1bOP"
    F2 = "\x1bOQ"
    F3 = "\x1bOR"
    F4 = "\x1bOS"
    F5 = "\x1b[15~"
    F6 = "\x1b[17~"
    F7 = "\x1b[18~"
    F8 = "\x1b[19~"
    F9 = "\x1b[20~"
    F10 = "\x1b[21~"
    F11 = "\x1b[23~"
    F12 = "\x1b[24~"

    CTRL_C = "\x03"
    CTRL_D = "\x04"
    CTRL_L = "\x0c"
    CTRL_Z = "\x1a"

    @staticmethod
    def ctrl(letter: str) -> str:
        """Ctrl+<letter> as a single byte. `letter` must be ASCII a-z."""
        ch = letter.lower()
        if len(ch) != 1 or not ("a" <= ch <= "z"):
            msg = f"Key.ctrl expects one ASCII letter a-z, got {letter!r}"
            raise ValueError(msg)
        return chr(ord(ch) - ord("a") + 1)

    @staticmethod
    def alt(letter: str) -> str:
        """Alt+<letter> as ESC + letter."""
        if len(letter) != 1:
            msg = f"Key.alt expects a single character, got {letter!r}"
            raise ValueError(msg)
        return "\x1b" + letter


# --------------------------------------------------------------------------- #
# Wait predicate
# --------------------------------------------------------------------------- #


Pattern: TypeAlias = str | re.Pattern[str] | Callable[["Snapshot"], bool]


class WaitTimeout(TimeoutError):
    """Raised when `Tui.wait_for(...)` exceeds its deadline."""


def _build_predicate(pattern: Pattern) -> Callable[[Snapshot], bool]:
    if isinstance(pattern, str):
        return lambda snap: pattern in snap.text
    if isinstance(pattern, re.Pattern):
        return lambda snap: pattern.search(snap.text) is not None
    return pattern


def _wrap_styled(raw_row: list) -> list[StyledCell]:
    return [
        StyledCell(
            char=c.character,
            fg=c.fgcolor,
            bg=c.bgcolor,
            bold=c.bold,
            italic=c.italic,
            underline=c.underline,
            inverse=c.inverse,
        )
        for c in raw_row
    ]


# --------------------------------------------------------------------------- #
# Tui
# --------------------------------------------------------------------------- #


class Tui:
    """A single spawned PTY-backed process.

    Get one from `TuiManager.spawn(...)`. Sync methods release the GIL on the
    underlying Rust call; async methods return native asyncio coroutines that
    are driven by the pyo3-async-runtimes tokio reactor — no thread pool hop.

    There is no force-kill path today. `interrupt()` sends Ctrl+C, which most
    cooperative programs respect; `with` blocks will interrupt on exit.
    """

    __slots__ = ("_raw",)

    def __init__(self, raw: _RawTuiInstance) -> None:
        self._raw = raw

    # -- identity / shape ---------------------------------------------------

    @property
    def id(self) -> uuid.UUID:
        return uuid.UUID(self._raw.id)

    @property
    def command(self) -> str:
        return self._raw.command

    @property
    def args(self) -> tuple[str, ...]:
        return tuple(self._raw.args)

    @property
    def size(self) -> Size:
        return Size(rows=self._raw.rows, cols=self._raw.cols)

    @property
    def scrollback_limit(self) -> int:
        return self._raw.scrollback_limit

    # -- writing ------------------------------------------------------------

    def write(self, data: str) -> Self:
        """Send `data` to the PTY exactly. Returns `self` for chaining."""
        self._raw.write(data)
        return self

    def send(self, *parts: str) -> Self:
        """Concatenate and send. Mix `Key` members with literal text freely."""
        if parts:
            self._raw.write("".join(parts))
        return self

    def enter(self, text: str = "") -> Self:
        """Send `text` followed by Enter."""
        self._raw.write(text + Key.ENTER)
        return self

    def interrupt(self) -> Self:
        """Send Ctrl+C."""
        self._raw.write(Key.CTRL_C)
        return self

    # -- reading ------------------------------------------------------------

    def viewport(self) -> list[str]:
        """Current viewport as a list of lines."""
        return self._raw.read_viewport()

    def scrollback(self) -> list[str]:
        """Lines that have scrolled off the viewport, oldest first."""
        return self._raw.read_scrollback()

    def text(self) -> str:
        """Current viewport joined with newlines."""
        return "\n".join(self._raw.read_viewport())

    def snapshot(self) -> Snapshot:
        """Immutable point-in-time view of viewport + scrollback."""
        full = self._raw.read_full()
        return Snapshot(
            viewport=tuple(full.viewport),
            scrollback=tuple(full.scrollback),
            size=self.size,
        )

    def read(self, *, timeout: float | None = None) -> list[str]:
        """Read the viewport.

        With `timeout=None` (the default), returns immediately.
        With `timeout` set, blocks up to that many seconds waiting for output.
        """
        if timeout is None:
            return self._raw.read_viewport()
        return self._raw.read_blocking(int(timeout * 1000))

    def chars(self) -> NDArray[np.uint32]:
        """Per-cell Unicode codepoints of the viewport, shape `(rows, cols)`."""
        return self._raw.read_chars_array()

    def styled_cells(self) -> list[list[StyledCell]]:
        """Per-cell styling for the viewport, indexed as `[row][col]`."""
        return [_wrap_styled(row) for row in self._raw.read_styled_cells()]

    # -- waits --------------------------------------------------------------

    def wait_for(
        self,
        pattern: Pattern,
        *,
        timeout: float = 5.0,
        poll: float = 0.05,
    ) -> Snapshot:
        """Block until the viewport matches `pattern`.

        `pattern` may be a substring, a compiled `re.Pattern`, or a callable
        that takes a `Snapshot` and returns a bool. Returns the first matching
        snapshot. Raises `WaitTimeout` on expiry.
        """
        check = _build_predicate(pattern)
        deadline = time.monotonic() + timeout
        while True:
            snap = self.snapshot()
            if check(snap):
                return snap
            if time.monotonic() >= deadline:
                raise WaitTimeout(
                    f"{self.command!r} did not match {pattern!r} within {timeout:.2f}s"
                )
            time.sleep(poll)

    # -- async I/O ----------------------------------------------------------

    async def awrite(self, data: str) -> None:
        """Native asyncio-awaitable PTY write."""
        await self._raw.write_async(data)

    async def asend(self, *parts: str) -> None:
        if parts:
            await self._raw.write_async("".join(parts))

    async def aenter(self, text: str = "") -> None:
        await self._raw.write_async(text + Key.ENTER)

    async def aread(self, *, timeout: float | None = None) -> list[str]:
        """Native asyncio-awaitable viewport read."""
        if timeout is None:
            return await self._raw.read_viewport_async()
        return await self._raw.read_blocking_async(int(timeout * 1000))

    async def asnapshot(self) -> Snapshot:
        """Native asyncio-awaitable snapshot."""
        full = await self._raw.read_full_async()
        return Snapshot(
            viewport=tuple(full.viewport),
            scrollback=tuple(full.scrollback),
            size=self.size,
        )

    async def achars(self) -> NDArray[np.uint32]:
        return await self._raw.read_chars_array_async()

    async def astyled_cells(self) -> list[list[StyledCell]]:
        rows = await self._raw.read_styled_cells_async()
        return [_wrap_styled(row) for row in rows]

    async def await_for(
        self,
        pattern: Pattern,
        *,
        timeout: float = 5.0,
        poll: float = 0.05,
    ) -> Snapshot:
        check = _build_predicate(pattern)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            snap = await self.asnapshot()
            if check(snap):
                return snap
            if loop.time() >= deadline:
                raise WaitTimeout(
                    f"{self.command!r} did not match {pattern!r} within {timeout:.2f}s"
                )
            await asyncio.sleep(poll)

    # -- protocol -----------------------------------------------------------

    def __str__(self) -> str:
        return self.text()

    def __repr__(self) -> str:
        return (
            f"Tui(id={self.id}, command={self.command!r}, "
            f"args={list(self.args)!r}, size={self.size!r})"
        )

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        try:
            self.interrupt()
        except Exception:
            # Best-effort: the channel may already be gone.
            pass

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        try:
            await self._raw.write_async(Key.CTRL_C)
        except Exception:
            pass


# --------------------------------------------------------------------------- #
# TuiManager
# --------------------------------------------------------------------------- #


class TuiManager:
    """Spawn and track PTY-backed processes.

    Construct once and reuse. `spawn` accepts the command followed by any
    positional args:

        with TuiManager() as mgr:
            tui = mgr.spawn("vim", "-u", "NONE")
    """

    __slots__ = ("_raw",)

    def __init__(self) -> None:
        self._raw = _RawTuiManager()

    def spawn(
        self,
        command: str,
        *args: str,
        scrollback_lines: int = 10_000,
    ) -> Tui:
        """Spawn `command` with the given positional args."""
        raw = self._raw.spawn(command, list(args), scrollback_lines)
        return Tui(raw)

    def spawn_argv(
        self,
        argv: Iterable[str],
        *,
        scrollback_lines: int = 10_000,
    ) -> Tui:
        """Spawn from a pre-built argv. First element is the command."""
        argv_list = list(argv)
        if not argv_list:
            raise ValueError("spawn_argv requires at least one element")
        command, *rest = argv_list
        raw = self._raw.spawn(command, rest, scrollback_lines)
        return Tui(raw)

    def list(self) -> list[Tui]:
        """All currently tracked instances."""
        return [Tui(raw) for raw in self._raw.list()]

    def __iter__(self) -> Iterator[Tui]:
        return iter(self.list())

    def __len__(self) -> int:
        return len(self._raw.list())

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        for tui in self.list():
            try:
                tui.interrupt()
            except Exception:
                pass

    def __repr__(self) -> str:
        return f"TuiManager(instances={len(self)})"
