"""High-level async API for the `tui` PTY-backed terminal manager.

Spawn child processes attached to real pseudo-terminals, drive them with
keystrokes, and observe their VT100-rendered viewport. The whole I/O surface is
async: every method that touches the terminal is a coroutine you must `await`.

    async with Tui("python", "-q") as t:
        await t.enter("print('hi')")
        snap = await t.wait_for("hi", timeout=2.0)

The awaitables are native asyncio coroutines bridged from Rust via
pyo3-async-runtimes, with no thread-pool hop. The only synchronous surface is
construction and the cached accessors: `id`, `command`, `args`, `size`,
`is_alive`, `exit_code`. Everything else (`send`, `enter`, `read`, `viewport`,
`text`, `snapshot`, `wait_for`, `resize`, `kill`, `close`) is a coroutine.

In a Jupyter notebook the cell already runs in an event loop, so drive a
terminal across cells without `async with`: construct `t = Tui(...)`, `await`
its methods cell by cell, and `await t.close()` when done. Evaluating
`await t.snapshot()` as the last expression in a cell renders the screen in
color via `Snapshot._repr_html_`.

Every spawned terminal auto-shows in the web dashboard. The first `Tui(...)`
binds a process-global producer, so running `nix run .#tui-dashboard` (it
watches `socket_dir()`) renders this process's terminals with no explicit
`tui.publish()`. Opt out by setting `IX_TUI_AUTOPUBLISH=0`.

The public surface:

    Tui             a single spawned process; the workhorse handle
    Snapshot        an immutable read-time view of one process
    Size            (rows, cols) terminal size
    Key             common keystrokes as ANSI byte strings, with .ctrl/.alt
    StyledCell      one viewport cell: character + VT100 attributes
    Color           a cell color: None (default), int (palette), or (r, g, b)
    WaitTimeout     raised by `Tui.wait_for(...)` when nothing matched in time
"""

from __future__ import annotations

import asyncio
import os
import re
import uuid
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from enum import StrEnum
from html import escape as _html_escape
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
    ensure_published as _raw_ensure_published,
    publish as _raw_publish,
    serve as _raw_serve,
    socket_dir as socket_dir,
)

__all__ = [
    "DARK_THEME",
    "DEFAULT_THEME",
    "LIGHT_THEME",
    "RGB",
    "Color",
    "Dashboard",
    "Key",
    "Pattern",
    "Publisher",
    "Size",
    "Snapshot",
    "StyledCell",
    "Theme",
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


# --------------------------------------------------------------------------- #
# HTML rendering (Jupyter `_repr_html_`)
# --------------------------------------------------------------------------- #


#: A concrete 24-bit color as `(r, g, b)`, each component `0..=255`.
RGB: TypeAlias = tuple[int, int, int]


def _hex_to_rgb(value: str) -> RGB:
    """Parse a `RRGGBB` or `#RRGGBB` hex color into `(r, g, b)`."""
    h = value.strip().lstrip("#")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))


def _build_xterm_cube() -> tuple[RGB, ...]:
    """Palette entries 16-255: the 6x6x6 color cube + 24-step grayscale ramp.

    These are the parts of the xterm 256-color palette that terminals do *not*
    theme (only indices 0-15 and the default fg/bg are themeable). Returned
    indexed from 0, so palette index `n` maps to `_XTERM_CUBE[n - 16]`.
    """
    cube_levels = (0, 95, 135, 175, 215, 255)
    out: list[RGB] = []
    for i in range(216):
        out.append((cube_levels[i // 36], cube_levels[(i // 6) % 6], cube_levels[i % 6]))
    for i in range(24):
        gray = 8 + i * 10
        out.append((gray, gray, gray))
    return tuple(out)


_XTERM_CUBE = _build_xterm_cube()


@dataclass(frozen=True, slots=True)
class Theme:
    """Colors for rendering a viewport to HTML.

    A terminal theme is the default `fg`/`bg` plus the 16 ANSI palette entries
    (`ansi`, indices 0-15). Extended palette colors (16-255) use the standard
    xterm cube and grayscale ramp, which terminals do not theme. `Color` values
    resolve against this: `None` -> `fg`/`bg`, an `int` in `0..16` -> `ansi`, a
    larger `int` -> the xterm cube, an `(r, g, b)` tuple -> itself.

    Build one from a ghostty theme file with `Theme.from_ghostty(...)`, or use
    the bundled `DARK_THEME` / `LIGHT_THEME`.
    """

    fg: RGB
    bg: RGB
    ansi: tuple[RGB, ...]  # exactly 16 entries (palette indices 0-15)
    name: str = "custom"

    @classmethod
    def from_ghostty(cls, source: str, *, name: str | None = None) -> Self:
        """Build a `Theme` from a ghostty theme (a file path or its text).

        Reads the `background = RRGGBB`, `foreground = RRGGBB`, and
        `palette = N=RRGGBB` lines that ghostty theme files under
        `ghostty/themes/` use; everything else (cursor, selection, comments) is
        ignored. Unspecified palette slots keep the bundled `DARK_THEME` color.
        """
        text = source
        if "\n" not in source:
            expanded = os.path.expanduser(source)
            if os.path.exists(expanded):
                with open(expanded, encoding="utf-8") as fh:
                    text = fh.read()
            elif "/" in source or source.startswith("~"):
                # Looks like a path but is not there: a typo'd path would
                # otherwise parse as (key-less) theme text and silently return
                # the defaults, so fail loudly instead.
                msg = f"ghostty theme file not found: {source}"
                raise FileNotFoundError(msg)

        fg, bg = DARK_THEME.fg, DARK_THEME.bg
        ansi = list(DARK_THEME.ansi)
        for raw in text.splitlines():
            line = raw.strip()
            # Whole-line comments start with `#`. A `#` inside a value is a hex
            # color (ghostty writes `background = #1e1e1e`), so do not treat it
            # as a comment delimiter; `_hex_to_rgb` strips a leading `#`.
            if not line or line.startswith("#"):
                continue
            key, sep, val = line.partition("=")
            if not sep:
                continue
            key, val = key.strip(), val.strip()
            if key == "background":
                bg = _hex_to_rgb(val)
            elif key == "foreground":
                fg = _hex_to_rgb(val)
            elif key == "palette":
                idx, eq, hexval = val.partition("=")
                if eq and idx.strip().isdigit():
                    slot = int(idx.strip())
                    if 0 <= slot < 16:
                        ansi[slot] = _hex_to_rgb(hexval)
        return cls(fg=fg, bg=bg, ansi=tuple(ansi), name=name or "ghostty")

    def resolve(self, color: Color, default: RGB) -> RGB:
        """Resolve a `Color` to concrete `(r, g, b)` under this theme."""
        if color is None:
            return default
        if isinstance(color, int):
            if 0 <= color < 16:
                return self.ansi[color]
            if 16 <= color < 256:
                return _XTERM_CUBE[color - 16]
            return default
        return (color[0], color[1], color[2])


#: Bundled dark theme (Catppuccin-Mocha-leaning: `#1e1e1e` bg, `#d4d4d4` fg).
DARK_THEME = Theme(
    fg=(0xD4, 0xD4, 0xD4),
    bg=(0x1E, 0x1E, 0x1E),
    ansi=(
        (0x00, 0x00, 0x00), (0xF3, 0x8B, 0xA8), (0xA6, 0xE3, 0xA1), (0xF9, 0xE2, 0xAF),
        (0x89, 0xB4, 0xFA), (0xCB, 0xA6, 0xF7), (0x94, 0xE2, 0xD5), (0xE0, 0xD8, 0xC0),
        (0x58, 0x5B, 0x70), (0xF3, 0x8B, 0xA8), (0xA6, 0xE3, 0xA1), (0xF9, 0xE2, 0xAF),
        (0x89, 0xB4, 0xFA), (0xCB, 0xA6, 0xF7), (0x94, 0xE2, 0xD5), (0xFA, 0xF0, 0xC8),
    ),
    name="dark",
)

#: Bundled light theme (`#f9f9f9` bg, `#2a2c33` fg).
LIGHT_THEME = Theme(
    fg=(0x2A, 0x2C, 0x33),
    bg=(0xF9, 0xF9, 0xF9),
    ansi=(
        (0x00, 0x00, 0x00), (0xDB, 0x3F, 0x39), (0x42, 0x93, 0x3E), (0x85, 0x55, 0x04),
        (0x32, 0x5E, 0xEE), (0x93, 0x00, 0x93), (0x0E, 0x70, 0xAE), (0x8F, 0x90, 0x96),
        (0x2A, 0x2C, 0x33), (0xDB, 0x3F, 0x39), (0x42, 0x93, 0x3E), (0x85, 0x55, 0x04),
        (0x32, 0x5E, 0xEE), (0x93, 0x00, 0x93), (0x0E, 0x70, 0xAE), (0xFF, 0xFF, 0xFF),
    ),
    name="light",
)

#: The theme `Snapshot._repr_html_` uses when none is passed. Reassign
#: `tui.DEFAULT_THEME` (e.g. to `Theme.from_ghostty(...)`) to restyle every
#: snapshot rendered afterward.
DEFAULT_THEME = DARK_THEME

# Berkeley Mono first (matches a common terminal font), then portable fallbacks.
_HTML_FONT = (
    "'Berkeley Mono','SF Mono',Menlo,Consolas,'DejaVu Sans Mono',monospace"
)


def _cell_css(cell: StyledCell, theme: Theme) -> str:
    """CSS declarations for one `StyledCell`, honoring inverse + attributes."""
    fg = theme.resolve(cell.fg, theme.fg)
    bg = theme.resolve(cell.bg, theme.bg)
    if cell.inverse:
        fg, bg = bg, fg
    css = [f"color:rgb({fg[0]},{fg[1]},{fg[2]})", f"background:rgb({bg[0]},{bg[1]},{bg[2]})"]
    if cell.bold:
        css.append("font-weight:700")
    if cell.italic:
        css.append("font-style:italic")
    if cell.underline:
        css.append("text-decoration:underline")
    return ";".join(css)


def _box_css(theme: Theme) -> str:
    """The container `<pre>` style: a flat, square terminal box in `theme`.

    No rounding or glow: it should read as a real screen, not UI chrome.
    """
    fg, bg = theme.fg, theme.bg
    return (
        "display:inline-block;"
        f"background:rgb({bg[0]},{bg[1]},{bg[2]});"
        f"color:rgb({fg[0]},{fg[1]},{fg[2]});"
        f"font-family:{_HTML_FONT};"
        "font-size:13px;line-height:1.2;padding:8px 10px;"
        "border:1px solid rgba(127,127,127,0.25);"
        "white-space:pre;overflow:auto"
    )


def _styled_grid_to_html(grid: tuple[tuple[StyledCell, ...], ...], theme: Theme) -> str:
    """Render a styled viewport as a colored monospace HTML block.

    Consecutive cells with identical styling collapse into one `<span>` so a
    full screen stays a few hundred spans, not one per cell.
    """
    lines: list[str] = []
    for row in grid:
        spans: list[str] = []
        run_css: str | None = None
        run: list[str] = []
        for cell in row:
            css = _cell_css(cell, theme)
            if css != run_css:
                if run:
                    spans.append(f'<span style="{run_css}">{_html_escape("".join(run))}</span>')
                run_css = css
                run = []
            run.append(cell.char)
        if run:
            spans.append(f'<span style="{run_css}">{_html_escape("".join(run))}</span>')
        lines.append("".join(spans))
    return f'<pre style="{_box_css(theme)}">{chr(10).join(lines)}</pre>'


def _plain_lines_to_html(lines: tuple[str, ...], theme: Theme) -> str:
    """Render plain viewport lines as a monospace block (no color captured)."""
    body = _html_escape("\n".join(lines))
    return f'<pre style="{_box_css(theme)}">{body}</pre>'


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
    """An immutable view of a Tui at a single point in time.

    In Jupyter, evaluating a snapshot as the last expression in a cell renders
    the viewport as a colored monospace block (`_repr_html_`). When `styled` is
    captured (the default for `Tui.snapshot()`), the render carries the real
    VT100 colors and attributes; otherwise it falls back to plain text.
    """

    viewport: tuple[str, ...]
    scrollback: tuple[str, ...]
    size: Size
    #: Per-cell styling for the viewport, `[row][col]`. Empty when the snapshot
    #: was taken with `styled=False` (e.g. the cheap polls inside `wait_for`).
    styled: tuple[tuple[StyledCell, ...], ...] = ()

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

    def __repr__(self) -> str:
        """Plain-text display for terminals, logs, and a notebook's `text/plain`.

        The dataclass default repr would expand every `StyledCell`, which for a
        full screen is tens of kilobytes of noise: an agent reading the cell
        output wants the screen, not the grid. Render the viewport inside a
        width-exact frame so columns, trailing space, and the real size are
        unambiguous, and summarize the styling instead of expanding it. The
        colored view is still available via `_repr_html_` / `to_html`.

        Width is counted in code points, not display columns, so a line with
        wide (CJK/emoji) glyphs can push the right border out; this is a
        cosmetic frame artifact in the plain-text view only.
        """
        rows, cols = self.size.rows, self.size.cols
        top = "┌" + "─" * cols + "┐"
        bottom = "└" + "─" * cols + "┘"
        body = "\n".join(f"│{line[:cols]:<{cols}}│" for line in self.viewport)
        notes = [f"{rows}x{cols}"]
        if self.scrollback:
            notes.append(f"+{len(self.scrollback)} scrollback")
        if self.styled:
            notes.append("styled")
        header = f"Snapshot {', '.join(notes)}"
        return f"{header}\n{top}\n{body}\n{bottom}" if body else f"{header}\n{top}\n{bottom}"

    def to_html(self, theme: Theme | None = None) -> str:
        """Render the viewport to a colored monospace HTML block.

        Uses `theme` if given, else the module-level `DEFAULT_THEME`. Falls back
        to a plain (uncolored) render when the snapshot carries no styling.
        """
        active = theme if theme is not None else DEFAULT_THEME
        if self.styled:
            return _styled_grid_to_html(self.styled, active)
        return _plain_lines_to_html(self.viewport, active)

    def _repr_html_(self) -> str:
        """Jupyter rich display: a colored render using `DEFAULT_THEME`."""
        return self.to_html()


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
# Auto-publish
# --------------------------------------------------------------------------- #

def _ensure_autopublish() -> None:
    """Bind the process-global dashboard producer once, on first `Tui(...)`.

    Spawned terminals then appear in `nix run .#tui-dashboard` with no explicit
    `tui.publish()`. Idempotency (bind at most once per process) and the
    `IX_TUI_AUTOPUBLISH=0` opt-out both live in the Rust `ensure_published`, so
    this stays a thin call into it rather than re-implementing either guard here.
    """
    _raw_ensure_published()


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
    of history (default 10,000). Pass the shape as `size=(rows, cols)` (the same
    spelling the `.size` accessor returns) or as granular `rows=`/`cols=`, but not
    both. A single process-wide tokio runtime drives every
    spawned PTY; each I/O method returns a native asyncio coroutine bridged
    through pyo3-async-runtimes, with no thread-pool hop. Construction and the
    shape accessors (`id`, `command`, `args`, `size`, `is_alive`, `exit_code`)
    are the only synchronous surface; everything else is a coroutine to await.

    The first `Tui(...)` auto-publishes this process to the web dashboard, so
    `nix run .#tui-dashboard` shows the terminal without an explicit
    `tui.publish()`. Set `IX_TUI_AUTOPUBLISH=0` to opt out.

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
        size: tuple[int, int] | Size | None = None,
        rows: int | None = None,
        cols: int | None = None,
        scrollback_lines: int | None = None,
    ) -> None:
        # `size=(rows, cols)` mirrors the `.size` accessor (a `Size` is also a
        # (rows, cols) iterable), so the shape can be read and set with the same
        # spelling; `rows=`/`cols=` stay as the granular form. Accept one or the
        # other, not both, so a conflicting pair is an error rather than a silent
        # winner.
        if size is not None:
            if rows is not None or cols is not None:
                raise TypeError("pass either size=(rows, cols) or rows=/cols=, not both")
            rows, cols = size
        _ensure_autopublish()
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

    @property
    def is_alive(self) -> bool:
        """Whether the child process is still running."""
        return self._raw.is_alive()

    @property
    def exit_code(self) -> int | None:
        """The exit code, or `None` while running or if killed by a signal."""
        return self._raw.exit_code()

    # -- writing ------------------------------------------------------------

    async def write(self, data: str) -> None:
        """Send `data` to the PTY.

        Like a real terminal, while the program has DECCKM (application cursor
        keys) enabled a bare cursor sequence (`ESC [ A`..`D`, `ESC [ H`/`F`) is
        rewritten to its `ESC O ...` form so arrows reach full-screen programs;
        every other byte passes through unchanged.
        """
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

    async def snapshot(self, *, styled: bool = True) -> Snapshot:
        """Immutable point-in-time view of viewport + scrollback.

        With `styled=True` (the default) the snapshot also captures per-cell
        styling, so evaluating it in a Jupyter cell renders the screen in color.
        Pass `styled=False` to skip that second read when you only need text
        (this is what the `wait_for` poll loop does).
        """
        scrollback, viewport = await self._raw.read_full_async()
        cells: tuple[tuple[StyledCell, ...], ...] = ()
        if styled:
            cells = tuple(tuple(row) for row in await self._raw.read_styled_cells_async())
        return Snapshot(
            viewport=tuple(viewport),
            scrollback=tuple(scrollback),
            size=self.size,
            styled=cells,
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
        snapshot (text-only; call `await t.snapshot()` for a colored render).
        Raises `WaitTimeout` on expiry.
        """
        check = _build_predicate(pattern)
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while True:
            # The predicate only inspects text, so skip the per-cell styling
            # read on every poll. The returned snapshot is therefore text-only;
            # call `await t.snapshot()` afterward if you want a colored render.
            snap = await self.snapshot(styled=False)
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
