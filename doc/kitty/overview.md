# kitty

`packages/kitty` is an encoder for the
[kitty terminal graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/):
it turns image bytes into the `APC _G ... ST` escape sequences that `kitty`,
`ghostty`, and `wezterm` understand, and nothing else. It does not open a
terminal, decode images, or do I/O: callers own those concerns and decide where
the returned `String` is written (`src/lib.rs:1-6`). It is a small Rust library
crate (workspace member, no flake output), consumed by `packages/git-log-pretty`.

## Two display models

- **Cursor placement** (`transmit` / `place`): draw at the cursor. Simplest, but
  pins the image to a screen position, so it cannot survive a program that
  repaints the screen (a pager, tmux, an editor).
- **Unicode placeholder** (`transmit_virtual` + `placeholder_row`): transmit the
  pixels once, then display the image wherever ordinary `U+10EEEE` placeholder
  cells are printed. Because those cells are normal text, a host that knows
  nothing about graphics reflows the image with the text, so it scrolls and pages
  cleanly (`src/lib.rs:8-16`).

## Public surface (`src/lib.rs`)

- `Image<'a>` (`:48`): `Png(&[u8])` (terminal decodes it, protocol `f=100`) or
  `Rgba { width, height, pixels }` (raw 8-bit RGBA, `f=32`).
- `Placement { cols: Option<u32>, rows: Option<u32>, move_cursor: bool }` (`:63`):
  cell-grid scaling (`c=`/`r=`); `move_cursor = false` emits `C=1` so the caller
  can lay out text around the image. `Default` is `move_cursor = true`.
- `transmit(&Image, id: Option<u32>, &Placement) -> String` (`:113`): transmit and
  display at the cursor; `Some(id)` stores the image so `place` can redraw it
  later without resending pixels (`a=T`).
- `place(id: u32, &Placement) -> String` (`:138`): redisplay a stored image, no
  payload (`a=p`).
- `transmit_virtual(&Image, id: u32, cols: u32, rows: u32) -> String` (`:161`):
  transmit and create an invisible virtual placement sized to a `cols`x`rows`
  cell box (`a=T,U=1`). `id` must be non-zero and fit 24 bits, because
  `placeholder_row` encodes it in a cell's 24-bit foreground color.
- `PLACEHOLDER: char` = `U+10EEEE` (`:149`).
- `placeholder_row(id: u32, row: u32, cols: u32) -> String` (`:191`): render one
  row of a virtual image as ordinary text: a `SGR 38;2;r;g;b` foreground escape
  carrying the `id`, then `cols` placeholder cells each tagged with a row and
  column diacritic, then `SGR 39` to reset only the foreground. Row/col indices
  past the diacritic table are dropped (clipping only an unreasonably large
  placement).
- `is_supported() -> bool` (`:91`): env-only best-effort detection. `false` under
  tmux/screen (`TMUX`/`STY`, which swallow graphics escapes); `true` for
  `KITTY_WINDOW_ID` or a `TERM`/`TERM_PROGRAM` advertising kitty/ghostty/wezterm.
  A `true` is a hint, not a guarantee, so callers should still offer an opt-out.

## Encoding details

- Payloads are base64 (`base64` is the sole dependency, `Cargo.toml:12`) and
  chunked: the protocol caps each command's base64 at 4096 bytes (`MAX_CHUNK`,
  `:44`), so `encode_chunks` (`:247`) splits into continuation commands. The first
  chunk carries the real control keys plus `m=1`; middles are `m=1`; the last is
  `m=0`. A payload at or under the cap is one `m=0` command.
- Quiet mode `q=2` is set on transmits so the terminal sends no acknowledgement
  that could corrupt a host program's output.
- The row/column diacritics come from kitty's `gen/rowcolumn-diacritics.txt`,
  vendored as `src/rowcolumn-diacritics.txt` and parsed once into a `Vec<char>`
  indexed by number (`diacritics`, `:217`). Index 0 is COMBINING OVERLINE
  (`U+0305`), index 1 is COMBINING VERTICAL LINE ABOVE (`U+030D`).

## Build

`package.nix` is `{ id = "kitty"; inRustWorkspace = true; passthruTests = true; }`:
a library, no flake output. The unit tests (`src/lib.rs:284`) assert the control
fields and chunk boundaries for PNG and RGBA, the virtual-placement keys, the
foreground-encoded id in placeholder rows, and that the diacritic table matches
the kitty spec.
