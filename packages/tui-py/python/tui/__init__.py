"""High-level Python API for the `tui` PTY-backed terminal manager.

Spawn child processes attached to real pseudo-terminals, drive them with
keystrokes, and observe their VT100-rendered viewport. The public surface is:

    Tui             a single spawned process; the workhorse handle
    Snapshot        an immutable read-time view of one process
    Size            (rows, cols) terminal size
    Key             common keystrokes as ANSI byte strings, with .ctrl/.alt
    StyledCell      one viewport cell: character + VT100 attributes
    Color           a cell color: None (default), int (palette), or (r, g, b)
    WaitTimeout     raised by `Tui.wait_for(...)` when nothing matched in time

The interface is async-only. Every I/O method is a coroutine that returns a
native asyncio-awaitable from the Rust side (via pyo3-async-runtimes); no
thread-pool hop is involved. Construction and the shape accessors (`id`,
`command`, `size`, `is_alive`, `exit_code`) are the only synchronous surface.
"""

from __future__ import annotations

import asyncio
import re
import uuid
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from enum import StrEnum
from types import TracebackType
from typing import Self, TypeAlias

import numpy as np
from numpy.typing import NDArray

from ._tui import (
    Dashboard as _RawDashboard,
    Publisher as _RawPublisher,
    StyledCell as StyledCell,
    TuiInstance as _RawTuiInstance,
    __version__,
    publish as _raw_publish,
    serve as _raw_serve,
    socket_dir as socket_dir,
)

__all__ = [
    "Color",
    "Dashboard",
    "Key",
    "Pattern",
    "Publisher",
    "Size",
    "Snapshot",
    "StyledCell",
    "Tui",
    "WaitTimeout",
    "__version__",
    "publish",
    "serve",
    "socket_dir",
]


# --------------------------------------------------------------------------- #
# Value types
# --------------------------------------------------------------------------- #


#: A VT100 cell color: `None` is the terminal default, an `int` in `0..=255` is
#: a palette index, and an `(r, g, b)` tuple is 24-bit truecolor. Read off a
#: `StyledCell` via `cell.fg` / `cell.bg`.
Color: TypeAlias = int | tuple[int, int, int] | None


@dataclass(frozen=True, slots=True)
class Size:
    """Terminal dimensions, in cells."""

    rows: int
    cols: int

    def __iter__(self) -> Iterator[int]:
        yield self.rows
        yield self.cols


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


# --------------------------------------------------------------------------- #
# Tui
# --------------------------------------------------------------------------- #


class Tui:
    """A single spawned PTY-backed process, driven asynchronously.

    Construct it with the command and its args, then drive it inside an
    `async with` block:

        async with Tui("python", "-q") as tui:
            await tui.enter("1 + 2")
            snap = await tui.wait_for("3", timeout=2.0)

    The terminal opens at `rows` x `cols` (default 80x24) with `scrollback_lines`
    of history (default 10,000). A single process-wide tokio runtime drives every
    spawned PTY; each I/O method returns a native asyncio coroutine bridged
    through pyo3-async-runtimes, with no thread-pool hop. Construction and the
    shape accessors (`id`, `command`, `args`, `size`, `is_alive`, `exit_code`)
    are the only synchronous surface.

    `kill()` sends SIGKILL; `interrupt()` sends a cooperative Ctrl+C; `close()`
    force-kills and drops the terminal from `list_all()`. `async with` blocks
    call `close()` on exit, so an editor or REPL that ignores Ctrl+C still goes
    away.
    """

    __slots__ = ("_raw",)

    def __init__(
        self,
        command: str,
        *args: str,
        rows: int | None = None,
        cols: int | None = None,
        scrollback_lines: int | None = None,
    ) -> None:
        self._raw = _RawTuiInstance(command, list(args), rows, cols, scrollback_lines)

    @classmethod
    def _from_raw(cls, raw: _RawTuiInstance) -> Self:
        self = cls.__new__(cls)
        object.__setattr__(self, "_raw", raw)
        return self

    @classmethod
    def list_all(cls) -> list[Self]:
        """All Tui instances currently alive in this process."""
        return [cls._from_raw(raw) for raw in _RawTuiInstance.list_all()]

    # -- identity / shape (synchronous) -------------------------------------

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

    def is_alive(self) -> bool:
        """Whether the child process is still running."""
        return self._raw.is_alive()

    @property
    def exit_code(self) -> int | None:
        """The exit code, or `None` while running or if killed by a signal."""
        return self._raw.exit_code()

    # -- writing ------------------------------------------------------------

    async def write(self, data: str) -> None:
        """Send `data` to the PTY exactly."""
        await self._raw.write_async(data)

    async def send(self, *parts: str) -> None:
        """Concatenate and send. Mix `Key` members with literal text freely."""
        if parts:
            await self._raw.write_async("".join(parts))

    async def enter(self, text: str = "") -> None:
        """Send `text` followed by Enter."""
        await self._raw.write_async(text + Key.ENTER)

    async def interrupt(self) -> None:
        """Send Ctrl+C. Cooperative: a program that traps SIGINT ignores it."""
        await self._raw.write_async(Key.CTRL_C)

    # -- reading ------------------------------------------------------------

    async def read(self, *, timeout: float | None = None) -> list[str]:
        """Read the viewport.

        With `timeout=None` (the default), returns immediately. With `timeout`
        set, blocks up to that many seconds waiting for output.
        """
        if timeout is None:
            return await self._raw.read_viewport_async()
        return await self._raw.read_blocking_async(int(timeout * 1000))

    async def viewport(self) -> list[str]:
        """Current viewport as a list of lines."""
        return await self._raw.read_viewport_async()

    async def scrollback(self) -> list[str]:
        """Lines that have scrolled off the viewport, oldest first."""
        return await self._raw.read_scrollback_async()

    async def text(self) -> str:
        """Current viewport joined with newlines."""
        return "\n".join(await self._raw.read_viewport_async())

    async def snapshot(self) -> Snapshot:
        """Immutable point-in-time view of viewport + scrollback."""
        scrollback, viewport = await self._raw.read_full_async()
        return Snapshot(
            viewport=tuple(viewport),
            scrollback=tuple(scrollback),
            size=self.size,
        )

    async def chars(self) -> NDArray[np.uint32]:
        """Per-cell Unicode codepoints of the viewport, shape `(rows, cols)`."""
        return await self._raw.read_chars_array_async()

    async def styled_cells(self) -> list[list[StyledCell]]:
        """Per-cell styling for the viewport, indexed as `[row][col]`."""
        return await self._raw.read_styled_cells_async()

    # -- waits --------------------------------------------------------------

    async def wait_for(
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
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            snap = await self.snapshot()
            if check(snap):
                return snap
            if loop.time() >= deadline:
                raise WaitTimeout(
                    f"{self.command!r} did not match {pattern!r} within {timeout:.2f}s"
                )
            await asyncio.sleep(poll)

    # -- lifecycle ----------------------------------------------------------

    async def resize(self, rows: int, cols: int) -> None:
        """Resize the terminal, delivering SIGWINCH to the child.

        Visible from every handle to the same process.
        """
        await self._raw.resize_async(rows, cols)

    async def wait(self, timeout: float | None = None) -> int | None:
        """Block until the child exits; return its exit code.

        `None` means the process was terminated by a signal (it has no exit
        code). Raises `WaitTimeout` if `timeout` seconds pass first.
        """
        if timeout is None:
            return await self._raw.wait_async()
        try:
            return await asyncio.wait_for(self._raw.wait_async(), timeout)
        except TimeoutError as exc:
            raise WaitTimeout(f"{self.command!r} still running after {timeout}s") from exc

    async def kill(self) -> None:
        """Force-terminate the child with SIGKILL. A no-op if already exited."""
        await self._raw.kill_async()

    async def close(self) -> None:
        """Force-kill the child and stop tracking it.

        Drops the terminal from `Tui.list_all()` and the dashboard. This is what
        `async with` blocks call on exit, so an editor or REPL that ignores
        Ctrl+C still goes away.
        """
        await self._raw.close_async()

    # -- protocol -----------------------------------------------------------

    def __repr__(self) -> str:
        return (
            f"Tui(id={self.id}, command={self.command!r}, "
            f"args={list(self.args)!r}, size={self.size!r})"
        )

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        try:
            await self._raw.close_async()
        except Exception:
            # Best-effort: the child may already be gone.
            pass


# --------------------------------------------------------------------------- #
# Web dashboard
# --------------------------------------------------------------------------- #


class Dashboard:
    """A running web dashboard that mirrors every live `Tui` in this process.

    The server, the Loro CRDT document, and the SSE stream all live in Rust; a
    background poll loop samples each terminal's viewport into the document and
    streams updates to connected browsers. Open `url` to watch the grid. Stop
    with `await stop()`, or use the handle as an async context manager.

        async with await serve() as dash:
            dash.open()
            ...
    """

    __slots__ = ("_raw",)

    def __init__(self, raw: _RawDashboard) -> None:
        self._raw = raw

    @property
    def url(self) -> str:
        """The URL to open in a browser."""
        return self._raw.url

    @property
    def addr(self) -> str:
        """The bound `host:port`, with the resolved port when `port=0`."""
        return self._raw.addr

    def open(self) -> Self:
        """Open the dashboard in the default browser."""
        import webbrowser

        webbrowser.open(self.url)
        return self

    async def stop(self) -> None:
        """Stop the server and its poll loop. Idempotent."""
        await self._raw.stop()

    def __repr__(self) -> str:
        return f"Dashboard(url={self.url!r})"

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self._raw.stop()


async def serve(
    host: str = "127.0.0.1",
    port: int = 8080,
    *,
    poll: float = 0.1,
    open_browser: bool = False,
) -> Dashboard:
    """Start the web dashboard for every `Tui` alive in this process.

    `host` must be an IP literal (`127.0.0.1` for local only, `0.0.0.0` to
    expose on the network); a hostname is not resolved. Pass `port=0` to bind an
    ephemeral port and read it back from `Dashboard.url`. `poll` is the viewport
    sampling interval in seconds. The server runs in background threads owned by
    Rust; await this to get the handle.
    """
    raw = await _raw_serve(host, port, max(1, int(poll * 1000)))
    dashboard = Dashboard(raw)
    if open_browser:
        dashboard.open()
    return dashboard


# --------------------------------------------------------------------------- #
# Producer (multi-process dashboard)
# --------------------------------------------------------------------------- #


class Publisher:
    """A running producer that exposes this process's terminals over a unix
    socket for the standalone `tui-dashboard` aggregator.

    Many processes can publish at once; the aggregator discovers each socket in
    the shared directory and renders every producer in one grid, so several
    agents share a single dashboard URL instead of each starting their own
    server. Each terminal appears under this process's `producer_id`. Stop with
    `await stop()`, or use the handle as an async context manager.

        async with await publish() as pub:
            ...
    """

    __slots__ = ("_raw",)

    def __init__(self, raw: _RawPublisher) -> None:
        self._raw = raw

    @property
    def path(self) -> str:
        """The unix socket path this producer is bound to."""
        return self._raw.path

    @property
    def producer_id(self) -> str:
        """This process's scope on the aggregated dashboard."""
        return self._raw.producer_id

    async def stop(self) -> None:
        """Stop streaming and unlink the socket. Idempotent."""
        await self._raw.stop()

    def __repr__(self) -> str:
        return f"Publisher(path={self.path!r})"

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        await self._raw.stop()


async def publish(path: str | None = None, *, poll: float = 0.1) -> Publisher:
    """Publish every `Tui` alive in this process over a unix socket.

    With `path` unset the socket lands in the discovery directory
    (`socket_dir()`) under a per-process name, where the `tui-dashboard`
    aggregator finds it. Run that aggregator separately to watch every
    publishing process in one browser grid. `poll` is the sampling interval in
    seconds. Await this to get the handle.
    """
    raw = await _raw_publish(path, max(1, int(poll * 1000)))
    return Publisher(raw)
