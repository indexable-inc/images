# tui-py

`packages/tui-py` is the Python binding for [tui](../tui/overview.md): spawn and
control PTY-backed programs from Python with full vt100 emulation, scrollback,
NumPy cell access, an in-process web dashboard, and a Playwright-style harness for
driving interactive coding agents. PyPI distribution name `ix-tui`, import name
`tui` (`README.md:14`). The API is async-only and built on
[pyo3-async-runtimes](https://docs.rs/pyo3-async-runtimes): every I/O method is a
native asyncio coroutine bridged from a Rust future with no thread-pool hop.

## Three layers

1. **The PyO3 extension `tui._tui`** (`src/lib.rs`): a `cdylib`
   (`Cargo.toml:9`) over the `tui` crate built with the `pyo3`, `dashboard`, and
   `publish` features (`Cargo.toml:20`). It is thin: it exposes the low-level
   `TuiInstance`, `StyledCell`, `Dashboard`, `Publisher`, and the functions
   `serve`, `publish`, `ensure_published`, `socket_dir` (`src/lib.rs:21-30`).
   Blocking work releases the GIL via `Python::detach`; async methods return
   asyncio-awaitable coroutines (`src/manager.rs` doc).
2. **The Python package `tui`** (`python/tui/__init__.py`): the high-level,
   documented surface. It wraps `_tui.TuiInstance` in the ergonomic `Tui` class
   and adds value types (`Snapshot`, `Size`, `Theme`, `Key`, `Color`) and HTML
   rendering. This is what callers import.
3. **`tui.harness`** (`python/tui/harness.py`): the agent-driving layer.

## Layer 1: the extension (`src/`)

- `TuiInstance` (`src/manager.rs:30`, `frozen`): constructor `(command, args=None,
  rows=None, cols=None, scrollback_lines=None)` spawns into a single process-wide
  `tui::TuiManager` held in a `OnceLock` (`src/manager.rs:13`). Sync accessors
  (`id`, `command`, `args`, `rows`, `cols`, `scrollback_limit`, `is_alive`,
  `exit_code`); async coroutines `write_async`, `read_viewport_async`,
  `read_scrollback_async`, `read_full_async`, `read_blocking_async`,
  `read_chars_array_async` (a `numpy.uint32` array), `read_styled_cells_async`,
  `resize_async`, `kill_async`, `wait_async`, `close_async`.
- `StyledCell` (`src/types.rs:11`): `char`, `fg`/`bg` (pythonic `Color`: `None`,
  `int`, or `(r,g,b)`), `bold`/`italic`/`underline`/`inverse`.
- `Dashboard` + `serve(host, port, poll_ms)` (`src/dashboard.rs`): the
  Loro-backed web dashboard for the global manager; same engine as the Rust
  `tui::serve`.
- `Publisher` + `publish(path, poll_ms)` + `ensure_published(poll_ms)` +
  `socket_dir()` (`src/publish.rs`): the producer side (below).

## Layer 2: the Python API (`python/tui/__init__.py`)

`Tui` (`__init__.py:524`) is the workhorse. Construction and the cached accessors
(`id`, `command`, `args`, `size`, `is_alive`, `exit_code`) are synchronous;
everything else is a coroutine. Highlights:

- Input: `write(data)`, `send(*parts)`, `enter(text="")`, `interrupt()`
  (Ctrl+C).
- Reads: `read(timeout=None)`, `viewport()`, `scrollback()`, `text()`,
  `snapshot(styled=True) -> Snapshot`, `chars() -> NDArray[uint32]`,
  `styled_cells() -> list[list[StyledCell]]`.
- `wait_for(pattern, timeout) -> Snapshot` (`__init__.py:697`): poll until a
  substring, compiled `re.Pattern`, or `Snapshot` predicate matches; raises
  `WaitTimeout` on the deadline. (The returned snapshot is text-only; the poll
  loop skips the styling read.)
- Lifecycle: `resize`, `wait(timeout)`, `kill`, `close`; `async with` calls
  `close` on exit.
- `Tui.list_all()` (sync, `__init__.py:583`).

Value types: `Snapshot` (`__init__.py:343`, frozen: viewport, scrollback, size,
styled cells; supports `str()`, `in`, `.text`/`.full_text`, and a colored
`_repr_html_` for Jupyter), `Size` (`__init__.py:320`), `Key` (a `StrEnum` of
ANSI sequences with `.ctrl`/`.alt`, `__init__.py:423`), `Theme`
(`__init__.py:140`, default fg/bg + 16 ANSI; `Theme.from_ghostty(...)` parses a
ghostty theme file), the bundled `DARK_THEME`/`LIGHT_THEME`/`DEFAULT_THEME`,
`Color`, and `WaitTimeout`. `serve`/`Dashboard` and `publish`/`Publisher` are
re-exported wrappers over layer 1.

## Auto-publish to the dashboard

The first `Tui(...)` calls `ensure_published()` (`src/publish.rs:132`), which
binds one process-global producer on the discovery socket so spawned terminals
appear in the standalone aggregator with no explicit `tui.publish()`. It is
idempotent, skipped when `IX_TUI_AUTOPUBLISH=0`, and superseded by an explicit
`tui.publish(...)` so a process never exposes two producers
(`src/publish.rs:38,98`). The producer/consumer transport and the aggregator
live in the dashboard domain; the README and Python
docstrings invoke the aggregator as `nix run .#dashboard`
([dashboard](../dashboard/overview.md)), the registered flake output for the
aggregator.

## Layer 3: the agent harness (`python/tui/harness.py`)

`tui.harness` drives interactive coding agents (Claude Code, Codex) the way
Playwright drives a browser: `Tui` is the raw page, a harness adds
`launch`/`keyboard`/`wait_for_idle`/`content`/`expect`. Classes (`harness.py`):
`Agent` (base, `:112`), `Claude` (`:405`, grounded against Claude Code 2.1.x),
`Codex` (`:429`, quiescence-only), `Keyboard` (`:91`), `Gate` (`:72`, an
onboarding screen cleared on launch), `AgentAssertions` + `expect(agent)`
(`:449,502`). Idle detection is quiescence-first: a turn is done when the
viewport stops changing for `settle` seconds and a `busy_marker` substring is
absent (`harness.py` `_settle`/`wait_for_idle`). The point of driving the real
TUI (not `claude -p`) is observability: the session shows up live on the web
dashboard. The harness symbols are re-exported at the package top level
(`__init__.py` `__all__`).

## Build

`nix build .#tui-py` writes a PEP 427 wheel
(`ix_tui-<version>-cp311-abi3-manylinux_2_34_<arch>.whl`, `README.md:25`). Linux
only (`package.nix:11`); the cdylib comes from the shared cargo-unit graph
(`ix.rustWorkspace.units.libraries.tui_py`, `default.nix:13`) and
`wheel/mkwheel.py` packages it with the Python source. There is no PEP 517
backend and no maturin: `pip install .` is not supported (`README.md:21`). The
extension is abi3 (`pyo3/abi3-py311`), so one wheel loads on CPython 3.11+. For
macOS, the mcp bundles the cdylib straight from the workspace graph for a
cross-platform `import tui` (`package.nix:8`).
