# progress-style

`packages/progress-style` is a small library crate that owns the shared
[`indicatif`](https://docs.rs/indicatif) progress-bar and spinner styling for ix
command-line tools, so `search`, `dag-runner`, and future commands render the
same shape instead of each hand-rolling a template. No binary, no flake output.

## Purpose

One owner for the glyphs, colors, and templates every ix CLI uses to draw
progress (`src/lib.rs:1-7`). A caller picks a style here, then sets the per-run
label with `ProgressBar::set_prefix` and the per-run status with
`ProgressBar::set_message`; the template itself stays fixed.

## Public surface (`src/lib.rs`)

- `bar(accent: &str) -> ProgressStyle` (`:45`): a determinate progress bar: a
  green braille spinner, the caller's `{prefix}` label, a `pos/len` counter, the
  contiguous block bar, and elapsed time. `accent` is an `indicatif` color name
  applied to the filled run over a `blue` track (e.g. `"cyan"`, `"magenta"`) so
  callers can mark distinct phases. The returned style is reusable across bars.
- `spinner() -> ProgressStyle` (`:75`): an indeterminate spinner for work with no
  known total: a cyan braille spinner, a bold `{prefix}` label, a `{wide_msg}`
  status line, and dimmed elapsed time, sized for one task in a `MultiProgress`
  group.

Both return `indicatif::ProgressStyle`. The `# Panics` docs note they never panic
in practice: the templates are fixed shapes, and the internal `expect` only
guards against an edit that makes a template malformed (`:37-41`, `:67-69`); the
single unit test builds both to prove the templates parse (`:90-95`).

## Glyphs

- `BAR_CHARS = "█▉▊▋▌▍▎▏░"` (`:23`): a full block, seven fractional blocks, and a
  light-shade track, in `indicatif`'s `progress_chars` order (first fills a cell,
  last marks empty, middle render the partially-filled head from 7/8 down to
  1/8). The empty glyph is `░` not a space so the track stays visible.
- `TICK_CHARS = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "` (`:27`): a braille spinner wheel with a trailing
  blank frame so the final tick clears cleanly. Shared by `bar` and `spinner`.

Templates are built through `format!` (with doubled braces for literal indicatif
keys) rather than string literals to sidestep a clippy
`literal_string_with_formatting_args` false positive on indicatif's `{key:.style}`
syntax (`:46-50`, `:76-77`).

## Build and packaging

`package.nix`: `inRustWorkspace`, `passthruTests`. Library crate, no flake
output. Only dependency: `indicatif` (`Cargo.toml:12`).
