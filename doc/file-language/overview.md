# file-language

`packages/file-language` maps a file path, name, or extension to the source
`Language` it holds, with no grammar, parser, or highlighting dependencies. A
consumer that only needs "what language is this file" (a chunker, tokenizer, or
search ranker) can depend on it without pulling in a tree-sitter grammar
closure. It is a single Rust workspace library crate (`id = file-language`, no
flake output) with zero dependencies (`Cargo.toml`); [code-highlight](../code-highlight/overview.md)
layers the grammar-to-query mapping on top of this enum
(`packages/code-highlight/Cargo.toml:14`).

## Public surface (`src/lib.rs`)

- `Language` (`lib.rs:24`): a `#[non_exhaustive]` enum of the curated source
  languages (Rust, Python, JS/TS/TSX, Go, C/C++, C#, Java, Scala, Swift, Ruby,
  PHP, Lua, Haskell, Elixir, OCaml, HTML, CSS, JSON, TOML, YAML, SQL, Nix, Bash,
  Markdown, ...). The same variant set the highlighter understands; being
  `#[non_exhaustive]` keeps adding a language non-breaking for downstream
  `match`es.
- `Language::from_path(&Path) -> Option<Self>` (`lib.rs:121`): the usual entry
  point. Prefers a recognized full filename over the extension, so
  extension-less or misleading names (`Cargo.lock` -> TOML, `Gemfile` -> Ruby,
  `Rakefile` -> Ruby, `mix.exs` -> Elixir) resolve correctly.
- `Language::from_extension(&str) -> Option<Self>` (`lib.rs:139`): match a bare
  extension (case-insensitive).
- `Language::from_file_name(&str) -> Option<Self>` (`lib.rs:182`): match a known
  filename.
- `Language::ALL` (`lib.rs:85`): every variant, the iteration source for callers
  that build per-language tables (the highlighter's config cache).
- `Language::name(self) -> &'static str` (`lib.rs:197`): a stable, unique,
  lowercase name per language.

An unrecognized path yields `None`, which callers treat as unknown (plain text).
The crate is a library with no flake package and no CLI.
