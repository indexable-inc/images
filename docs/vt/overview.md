# vt

`packages/vt` is the VT engine: drive a terminal state machine with raw bytes and
snapshot its render state. It wraps [ghostty](https://ghostty.org/)'s
`libghostty-vt` C library in three layers, each its own crate/package under
`packages/vt/`:

| member | kind | role |
| --- | --- | --- |
| `ix-vt` | Rust lib crate (`nix build .#ix-vt`) | the safe, owned API: `Terminal`, `Snapshot`, `Cell`, `Cursor`. The only layer callers use. |
| `ix-vt-sys` | Rust `-sys` crate (no flake output) | mechanically generated raw FFI (`bindgen`) over `ghostty/vt.h`. No logic, no safety. |
| `libghostty-vt` | Nix/Zig package (`nix build .#libghostty-vt`) | ghostty's VT engine built as a standalone C library: the `.a`, the self-contained `.dylib`/`.so`, and the headers. |

[tui](../tui/overview.md) is the in-repo consumer: its engine thread owns an
`ix_vt::Terminal` (`packages/tui/src/actor/engine.rs`). The conversion from
`ix_vt` types to `tui`'s `Color`/`CursorShape`/`StyledCell` is in
`packages/tui/src/types.rs`.

## ix-vt: the safe wrapper (`ix-vt/src/lib.rs`)

`Terminal` (`ix-vt/src/lib.rs:285`) owns a `GhosttyTerminal` pointer and frees it
on drop. It is deliberately `!Send + !Sync`: libghostty-vt's terminal has thread
affinity, so the handle must stay on the thread that created it
(`ix-vt/src/lib.rs:289`). A caller needing it across threads pins it behind a
channel API (which is exactly what `tui`'s engine thread does); do not add an
`unsafe impl Send`.

Public API:

- `Terminal::new(rows, cols, scrollback) -> Result<Terminal>`
  (`ix-vt/src/lib.rs:312`) and `with_options(TerminalOptions { cols, rows,
  max_scrollback })` (`:325`). The `new` argument order reads as a screen size.
- `vt_write(&mut self, &[u8])` (`:338`): feed raw VT bytes (text + escapes).
- `resize(&mut self, rows, cols) -> Result<()>` (`:347`): both must be > 0.
- `scroll_viewport(ScrollViewport::{Top, Bottom, Delta(isize)})` (`:357`): move
  the viewport over scrollback. `render` always reads the active viewport, so
  reading history is scroll-up, render, scroll-back.
- `render(&self) -> Result<Snapshot>` (`:393`): capture an owned snapshot.
- `application_cursor_keys(&self) -> Result<bool>` (`:434`): query DECCKM (DEC
  private mode 1); `tui` reads this after each feed to pick the arrow-key form on
  write.

`Snapshot` (`ix-vt/src/lib.rs:245`) is fully owned and copied out of the C
structures, so it stays valid after the terminal is written to or dropped:
`cols`, `rows`, `viewport: Vec<Vec<Cell>>`, `scrollback: u64`, `cursor: Cursor`.

- `Cell` (`:186`): `ch: Option<char>` (base codepoint, `None` for empty),
  `combining: Vec<char>`, `style: Style`, and `fg`/`bg: Option<Rgb>` (resolved
  RGB with palette indices already looked up; `None` is the terminal default).
- `Style` (`:128`): the SGR booleans (`bold`, `italic`, `faint`, `blink`,
  `inverse`, `invisible`, `strikethrough`, `overline`), `underline: Option<u8>`,
  and the declared `fg_color`/`bg_color`/`underline_color: StyleColor` (before
  palette resolution).
- `StyleColor::{None, Palette(u8), Rgb(Rgb)}` (`:90`), `Rgb { r, g, b }` (`:66`).
- `Cursor` (`:227`): `visible`, `blinking`, `visual_style: CursorVisualStyle`,
  `viewport: Option<(col, row)>` (`None` when scrolled out of view).
- `CursorVisualStyle::{Bar, Block, Underline, BlockHollow}` (`:203`), the
  DECSCUSR shape.

`Error::{OutOfMemory, InvalidValue, OutOfSpace, Unknown(i32)}`
(`ix-vt/src/lib.rs:27`) wraps the non-success `GhosttyResult` codes; `check`
(`:55`) maps them.

## ix-vt-sys: the FFI (`ix-vt-sys/`)

A 1:1 binding of `ghostty/vt.h` produced by bindgen (`src/bindings.rs` is checked
in; `regen-bindings.sh` regenerates it). `src/lib.rs` just re-exports
`bindings::*`. It declares `links = "ghostty-vt"` (`Cargo.toml:9`) and its
`build.rs` links the **self-contained dynamic** library
(`cargo:rustc-link-lib=dylib=ghostty-vt`, `build.rs:39`), not the static archive:
the static `.a` does not bundle its C++ deps (libhighway, libsimdutf, libutfcpp),
while the dylib carries them, so one link directive suffices (`build.rs:9-13`).
The library directory is supplied out of band through the `IX_VT_GHOSTTY_LIB_DIR`
env var (`build.rs:25`), which the workspace sets for every unit
(`lib/rust/workspace.nix:215`) because a build-script `rustc-link-search` does
not propagate to the final per-unit Nix link.

## libghostty-vt: the C library (`libghostty-vt/`)

ghostty is consumed as a pinned source tree (the `ghostty` flake input, a fixed
commit, `flake.nix:146`), not a flake. The build recipe lives in
`lib/build/libghostty-vt.nix` (exposed as `ix.buildLibghosttyVt`) so the Rust
workspace can reuse the exact artifact when linking `ix-vt-sys`; the package's
`default.nix` is a thin wrapper plus a layout smoke test. It is a Zig build
(`-Demit-lib-vt=true`) with zon2nix-vendored dependencies, built with
`pkgs.zig_0_15` (the `requireZig` minor in `build.zig.zon`,
`flake.nix:139-149`, `lib/languages/zig.nix:14`). The output carries
`lib/libghostty-vt.a`, a versioned self-contained shared library, and
`include/ghostty/vt.h` + `include/ghostty/vt/`; the `layout` passthru test
asserts those exist (`libghostty-vt/default.nix:19-42`).

## Build wiring summary

```
nix build .#libghostty-vt  -> .a + .so/.dylib + headers (ghostty source, zig 0.15)
ix-vt-sys build.rs         -> links dylib=ghostty-vt from IX_VT_GHOSTTY_LIB_DIR
ix-vt                      -> safe wrapper over ix-vt-sys  (nix build .#ix-vt)
tui engine thread          -> owns an ix_vt::Terminal
```

`ix-vt`'s tests dlopen the dylib at runtime, so the workspace adds the
libghostty-vt lib dir to that unit's runtime inputs
(`lib/rust/workspace.nix:196-198`). `ix-vt` round-trip tests are in
`ix-vt/tests/round_trip.rs`.
