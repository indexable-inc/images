# reel

`packages/reel` records a terminal demo reel "as code": it drives a real CLI
session through the [tui](../tui/overview.md) PTY driver, samples the
VT-rendered grid of styled cells over time, rasterizes each frame to RGBA with a
flat palette and a vendored monospace face, and muxes the frames through ffmpeg
into an animated AVIF (with a WebP fallback). It emits a dark and a light variant
sized for a GitHub README `<picture>` element (`src/main.rs:1-15`). This is the
tool that generates the repo's top-of-README demo:
`docs/demo-{dark,light}.{avif,webp}` (root `README.md:13-19,46-54`). The clip is
not a screen recording; it is rendered from a recorded transcript of real
programs.

A Rust workspace crate (`Cargo.toml`); flake output `.#reel`. Its only repo
dependency is `tui` (`Cargo.toml:11-16`).

## Public surface

The binary is the surface. CLI flags (`src/main.rs:55-76`, all `--long`):

| flag | default | meaning |
| --- | --- | --- |
| `--out-dir` | `docs` | directory the `demo-{dark,light}.{avif,webp}` files are written into |
| `--width` | `880` | output width in px; height follows the recorded aspect ratio |
| `--font-size` | `30.0` | body font size in px before downscaling |
| `--cols` | `88` | terminal width in columns |
| `--rows` | `24` | terminal height in rows |
| `--fps` | `60` | AVIF capture frame rate; the WebP fallback is capped at 24 (`src/main.rs:100`) |

Output naming is fixed: `demo-<theme>.<ext>` where theme is `dark`/`light`
(`theme.rs:46-52`) and ext is `avif`/`webp` (`encode.rs:33-38`), so four files
per run (`src/main.rs:101-115`).

## Modules (`src/main.rs:35-40`)

- **`theme`** (`src/theme.rs`) - the two flat palettes (`DARK`, `LIGHT`) and
  `Palette::resolve`, which maps a tui `Color` to concrete RGB. `Color::Default`
  returns `None` so the caller substitutes the surface default
  (`theme.rs:69-75`); xterm-256 indices are computed (16-231 color cube,
  232-255 grayscale ramp, `theme.rs:77-108`).
- **`font`** (`src/font.rs`) - `FontSet`: four JetBrains Mono Nerd Font faces
  (regular/bold/italic/bold-italic) `include_bytes!`-embedded so a render never
  needs a system font and is identical in CI (`font.rs:14-17`). Caches rasterized
  body glyphs by `(char, face)`; `rasterize_at` bypasses the cache for the larger
  card text. The base font is SIL OFL 1.1 (`fonts/OFL.txt`).
- **`raster`** (`src/raster.rs`) - `Layout` (constant per-reel geometry derived
  from font metrics, `raster.rs:88-107`) and `render_frame`, which paints a flat
  window chrome (bar, three squared dots, hairline rule) plus either the terminal
  grid or a centered card onto an RGBA `Canvas`.
- **`scene`** (`src/scene.rs`) - the `Frame` enum (`Terminal { cells, cursor }`
  or `Card`), the `Action` script DSL, the demo `script(fps)`, and the
  `title_card`/`outro_card`.
- **`record`** (`src/record.rs`) - drives a real `bash` through tui and collects
  frames.
- **`encode`** (`src/encode.rs`) - streams rendered frames to ffmpeg.

## Recording flow (`src/record.rs`)

`record` spawns `bash --noprofile --norc -i` via `tui::TuiManager::spawn` with a
`SpawnConfig { rows, cols, scrollback_lines: 2000 }` (`record.rs:19-34`). Hidden
setup (not captured) sets a clean `PS1`, empty `PS2`, `TERM=xterm-256color`,
`HISTFILE=/dev/null`, then clears the screen (`record.rs:38-44`). Each scripted
`Action` is then replayed, capturing one `Frame` per wall-clock frame interval
via `term.read_styled_cells()` + `term.read_cursor()` (`record.rs:57-69`):

- `Type` reveals one char at a time, holding each for `frames_per_char` frames so
  typing reads at a steady ~18 chars/sec at any capture rate
  (`record.rs:47-48,80-91`).
- `Send` writes raw bytes (e.g. `\r`, `\x04` EOF) and captures one frame.
- `Hold(n)` captures `n` frames of the static screen.
- `WaitFor { needle, max }` captures until `term.read_viewport()` contains
  `needle` or `max` frames pass (`record.rs:103-112`) - used to wait for the
  Python `>>>` prompt before typing into the REPL.

Sampling on wall-clock cadence means the child's real output streams into the
capture: the recording shows actual programs running, not a faked transcript
(`record.rs:1-6`). The current script (`scene.rs:57-90`) runs `git-log-pretty
--no-pager`, then a live `python3 -q` REPL; both run offline so the clip is
reproducible. `main` bookends the recording with held title/outro cards
(2.2s/3.0s, `src/main.rs:86-93`).

## Rasterization (`src/raster.rs`)

Everything is flat: no gradients, shadows, or rounded corners (`raster.rs:1-8`).
`Layout::new` derives padding, chrome height, and cell size from the font metrics
(`raster.rs:91-106`); all frames in one reel share these dimensions.
`draw_terminal` paints, per cell: background fill (honoring `inverse` by swapping
ink/back), the glyph (face chosen by `FontSet::face_index(bold, italic)`),
underline, and an accent-colored cursor block with the underlying glyph redrawn
in the background color (`raster.rs:168-242`). Each cell is placed at one
monospace advance, so a wide glyph would overflow; the demo scenes are ASCII so
this never bites (`raster.rs:7-8`). `draw_card` centers a large title, optional
subtitle, optional footer (`raster.rs:245-297`).

## Encoding (`src/encode.rs`)

`encode` renders every frame at full size and pipes it to ffmpeg as raw RGBA
(`-f rawvideo -pix_fmt rgba`), then lanczos-downscales to `--width`
(`scale={w}:-2:flags=lanczos`); rendering full and downscaling supersamples text
for clean edges (`encode.rs:2-8,98-120`). Per-codec encoder args
(`encode.rs:41-72`):

- **AVIF** (primary): `libsvtav1 -crf 30 -preset 7 -pix_fmt yuv420p -loop 0 -an`.
  AV1 inter-frame compression makes the many identical hold frames nearly free,
  so a 60fps clip stays under GitHub's 10 MB image cap.
- **WebP** (fallback): `libwebp -q:v 50 -compression_level 6 -preset picture
  -loop 0 -an`, emitted at <=24 fps to stay small.

If ffmpeg dies mid-stream the broken-pipe write error is recorded but the code
still `wait()`s so ffmpeg's own exit status (the real cause) is reported
(`encode.rs:126-150`).

## Build and wiring (`default.nix`)

The bare binary is built via `ix.cargoUnit.selectBinaryWithTests`
(`default.nix:40-43`) then wrapped with `makeWrapper` to prepend a runtime PATH
(`default.nix:59-70`) because reel shells out by name while recording. Runtime
inputs (`default.nix:50-57`): `ffmpeg` (encode), `bashInteractive` (the driven
shell), `git` + `python3` (the demoed scene), and the repo's own `file-search`
and `git-log-pretty`, each built from the shared workspace unit graph so the
wrapper gets host-correct binaries without the repo overlay (`default.nix:8-27`).
The license is dual MIT + OFL because the binary embeds JetBrains Mono
(`default.nix:29-38`). A `printsHelp` passthru test asserts `reel --help` exits 0
and prints `Usage: reel` (`default.nix:72-90`).

Regenerate the README assets with `nix run .#reel` (writes
`docs/demo-{dark,light}.{avif,webp}`). reel must not be run by docs authoring
itself; the demo files are owned outputs and are not edited here.

## Gotchas

- The demo `script` (`scene.rs:57-90`) is hard-coded; changing the shown tools or
  pacing means editing it, and the outro card text (`scene.rs:104-109`) is
  likewise static.
- Frame count scales with `--fps` and the script's hold seconds; the RGBA frames
  are streamed (not all held in memory) but the `Vec<Frame>` of captured grids is
  (`src/main.rs:89-93`).
- See [tui](../tui/overview.md) for `TuiManager::spawn`, `SpawnConfig`
  (defaults 80x24, 10k scrollback; reel overrides to 2000), `read_styled_cells`,
  `read_cursor`, `read_viewport`, and the `StyledCell`/`Color` types reel renders.
