# tui

`packages/tui` is the repo's flagship PTY primitive: spawn and control multiple
interactive terminal programs (gdb, vim, a shell, a REPL) from one process as if
a human were typing, and read back a VT-rendered screen instead of a raw byte
stream. It is a Rust library crate (`name = "tui"`,
`packages/tui/Cargo.toml:2`), a workspace member with no standalone flake output;
it is consumed directly by [tui-node](../tui-node/overview.md),
[tui-py](../tui-py/overview.md), and `tap`'s integration tests, and indirectly
anywhere those bindings are used.

The crate-level mechanism (the actor mailbox, the dedicated VT engine thread,
the cursor-key rewrite, the initial-paint wait) is in
[internals](internals.md). This page is the public surface and how it builds.

## Module map (`src/lib.rs:38-61`)

- `manager` (`mod.rs`, `spawn.rs`, `reader.rs`): `TuiManager` and `TuiInstance`,
  the public entry points.
- `actor` (`mod.rs`, `engine.rs`): the per-child PTY actor task and the VT engine
  thread. Private; see [internals](internals.md).
- `types`: the plain value types (`SpawnConfig`, `StyledCell`, `Color`,
  `CursorPos`, `CursorShape`, `ExitState`, `FullOutput`).
- `slice`: `slice_2d` with `RowRange`/`ColRange` for sub-rectangle extraction.
- `error`: the `snafu`-derived `Error` enum and `Result` alias.
- `frame` + `publish` + `dashboard`: gated on the `publish`/`dashboard` features
  (below); the bridge from a live manager to dashboard panes.

## Public surface

`TuiManager` (`src/manager/mod.rs:249`):

- `new()` / `default()`: create a manager owning a fresh multi-threaded tokio
  runtime (`mod.rs:266`). Panics only if the runtime cannot be created.
- `spawn(command, args, SpawnConfig) -> Result<TuiInstance>` (`mod.rs:277`):
  open a PTY, launch the child, register the instance, and wait briefly for the
  first paint before returning.
- `list() -> Vec<TuiInstance>` (`mod.rs:290`), `get(&Uuid) -> Result<TuiInstance>`
  (`mod.rs:306`), `remove(&Uuid) -> Option<TuiInstance>` (`mod.rs:301`). Removal
  stops tracking; the actor keeps running until every handle drops, so kill
  first if you want the process gone.

`TuiInstance` (`src/manager/mod.rs:26`) is a cheap `Clone` handle; every clone
addresses the same process and carries its own clone of the runtime, so it keeps
working as long as held. Public fields: `id: Uuid`, `command`, `args`,
`spawned_at`, `scrollback_limit`. Every blocking method below has an `_async`
twin returning a future for callers already inside tokio.

- Shape: `rows()`, `cols()` (`mod.rs:51,57`), `cursor_shape() -> CursorShape`
  (`mod.rs:64`), `read_cursor() -> Result<CursorPos>` (`mod.rs:69`).
- Input: `write(&str)` (`mod.rs:106`). Applies the DECCKM cursor-key rewrite
  (see [internals](internals.md)); all other bytes pass through.
- Reads: `read_viewport() -> Vec<String>` (`mod.rs:115`),
  `read_scrollback()` (`mod.rs:121`), `read_full() -> FullOutput` (`mod.rs:127`),
  `read_blocking(timeout)` (polls the viewport until non-empty or timeout, then
  `NoOutputAvailable`, `mod.rs:133`), `read_chars() -> Vec<Vec<char>>`
  (`mod.rs:139`), `read_styled_cells() -> ndarray::Array2<StyledCell>`
  (`mod.rs:145`).
- Lifecycle: `exit_state() -> ExitState` (`mod.rs:154`), `is_alive()`
  (`mod.rs:160`), `wait(Option<Duration>) -> Option<ExitState>` (`mod.rs:169`,
  `None` on timeout), `kill()` (SIGKILL, `mod.rs:190`),
  `resize(rows, cols)` (resizes the kernel PTY window, which delivers `SIGWINCH`,
  and the emulator together; visible from every clone, `mod.rs:84`).

Value types (`src/types.rs`):

- `SpawnConfig { rows, cols, scrollback_lines }`, default 24x80 / 10,000 lines
  (`types.rs:122`).
- `StyledCell { character, fg, bg, bold, italic, underline, inverse }`
  (`types.rs:35`); an unwritten cell is a space with `Color::Default`.
- `Color::{Default, Indexed(u8), Rgb(u8,u8,u8)}` (`types.rs:10`), converted from
  `ix_vt::StyleColor`.
- `CursorShape::{Block, Underline, Bar}` (`types.rs:66`), from the engine's
  DECSCUSR state; blink and ghostty's hollow block collapse to `Block`.
- `ExitState::{Running, Exited(Option<i32>)}` (`types.rs:100`); `Exited(None)`
  means a signal killed it.
- `CursorPos { row, col, visible }` (viewport coordinates, 0-based, cursor
  scrolled off the viewport reports `(0,0)`).
- `FullOutput { scrollback, viewport }`.

`slice_2d(&[String], RowRange, ColRange) -> Result<Vec<String>>`
(`src/slice/core.rs:53`): extract a rectangular sub-region, 1-indexed inclusive,
`None` endpoints filled from the available extent. An empty input is `Ok(empty)`;
bad bounds are `InvalidRowRange`/`InvalidColRange`/`RowIndexOutOfBounds`/
`ColIndexOutOfBounds`.

## Errors (`src/error.rs:4`)

`Error` is a `snafu` enum: `ProcessSpawn`, `TuiNotFound` (the actor has exited /
the channel is closed), `WriteToTui`, `ReadFromTui`, `SignalTui`, `ResizeTui`,
`NoOutputAvailable`, `VtEngine`, the four slice-bounds variants, `ArrayConversion`,
and the feature-gated `Dashboard` / `Publish`. Under the `pyo3` feature the enum
also implements `From<Error> for pyo3::PyErr`, mapping each variant to a Python
exception class (`error.rs:78`); [tui-py](../tui-py/overview.md) turns that on.

## Features and the dashboard bridge

Three optional features (`packages/tui/Cargo.toml`), all off by default so the
core PTY library stays free of the HTTP/CRDT closure:

- `pyo3`: adds the `PyErr` conversion only. Used by `tui-py`.
- `publish`: producer side. `tui::publish(&manager, path, poll) -> Publisher`
  (`src/publish/mod.rs:32`) binds a Unix socket and streams the manager's
  terminals as NDJSON pane snapshots on a poll loop. `socket_path()` and
  `discovery_dir()` (re-exported from `dashboard-core`) give the per-process
  path.
- `dashboard`: in-process viewer. `tui::serve(&manager, addr, poll) -> Dashboard`
  (`src/dashboard/mod.rs:52`) binds the `dashboard-core` server and drives it
  from a poll loop over one manager, filing every terminal as a pane under the
  single `"local"` scope.

Both features re-export `dashboard-core` names (`Dashboard`, `Hub`, `serve_hub`,
`Pane`, `ProducerSnapshot`, `TerminalView`, `View`, `discovery_dir`,
`socket_path`, `src/lib.rs:50-55`). The only engine-bound code here is
`frame::collect_panes` (`src/frame/mod.rs:23`), which samples each terminal's
styled cells, encodes the screen as minimal ANSI SGR (`frame/sgr.rs`), and wraps
it as a `TerminalView`. Everything downstream (the Loro document, the SSE stream,
the browser page, the multi-process aggregator) lives in the
dashboard domain; see
[dashboard-core](../dashboard-core/overview.md). The poll and accept
loops run on the manager's own runtime (`runtime_handle`,
`src/manager/mod.rs:321`), so a dashboard started from a temporary runtime keeps
running after that runtime drops.

## How it builds

`packages/tui/package.nix` is just `{ id = "tui"; inRustWorkspace = true; }`: no
flake output, built only as a workspace library. It depends on
[ix-vt](../vt/overview.md) (`Cargo.toml:21`), so a build links the libghostty-vt
dylib that the workspace injects via `IX_VT_GHOSTTY_LIB_DIR`
(`lib/rust/workspace.nix:215`). Other deps: `pty-process` (PTY creation), `tokio`
(runtime), `ndarray` (the cell grid), `parking_lot` (registry lock), `uuid`,
`snafu`.

Note: the README still says "no runtime resize today"
(`packages/tui/README.md:143`), but `resize`/`resize_async` exist
(`src/manager/mod.rs:84,92`) and the bindings expose them; trust the source.
