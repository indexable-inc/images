# code-highlight

`packages/code-highlight` is a tree-sitter syntax highlighter that renders a
source string (or a line range) as ANSI-colored terminal text. It owns one job:
source plus a language hint (path or extension) into colored output, with a
plain-text fallback for unsupported languages or any failure, so a caller always
renders something. It is a single Rust workspace library crate (`id =
code-highlight`, no flake output); the search tooling
is its consumer (`packages/search/search/Cargo.toml`).

## Public surface (`src/lib.rs`)

- `highlight(path_or_lang, source, theme, color) -> String` (`lib.rs:426-432`):
  color a whole file. With `color = false` the output is the input unchanged
  (the `NO_COLOR` / non-TTY path).
- `highlight_lines(path_or_lang, source, start_line, num_lines, theme, color) ->
  String` (`lib.rs:447-490`): color the file, then slice a 1-based line window
  and prefix a right-aligned, dimmed line-number gutter (` │ ` separator). This
  is the snippet shape a search tool renders for context: highlight once with
  full-file context, then slice.
- `Theme` (`lib.rs:289-296`): `Dark` (default) or `Light`, so the caller can
  match the terminal background.
- `Language` is re-exported from [file-language](../file-language/overview.md)
  (`lib.rs:34`).

Both entry points resolve the language by trying the full path then a bare
extension (`detect`, `lib.rs:412-414`), and never error.

## Internals

- **Grammar dispatch** (`grammar_query`, `lib.rs:97-229`): a flat
  one-arm-per-`Language` table pairing a `tree-sitter-<lang>` grammar with its
  highlights query. TypeScript/TSX prepend the JavaScript highlights query (the
  TS query only adds type rules), and JS/TSX fold in the JSX query. Each grammar
  exports its query under a different constant name, hence the explicit table.
  An unmapped variant returns `None` -> plain text.
- **Capture names** (`HIGHLIGHT_NAMES`, `lib.rs:45-78`): the fixed conventional
  tree-sitter capture taxonomy; the index a grammar reports is the index into
  this slice. Dotted names fall back to their prefix (`function.method` ->
  `function`, `style_for`, `lib.rs:379-393`).
- **Theme** (`lib.rs:298-365`): colors come from the embedded
  `src/islands-theme.json`, the single source of truth shared with the
  base-profile Neovim colorscheme. `slot_for_name` maps each capture to a
  palette slot and italic flag; `parse_hex` reads `#RRGGBB` (alpha ignored).
- **Caching** (`CONFIGS`, `lib.rs:271-282`): each language's
  `HighlightConfiguration` is compiled once into a process-wide `LazyLock` map;
  a language whose query fails to compile maps to `None` (treated as
  unsupported) and the failure is printed once at first build
  (`build_config`, `lib.rs:245-263`).

The crate links ~30 grammar crates statically (`Cargo.toml`); no runtime deps.
There is no separate flake package and no CLI: it is consumed as a library.
