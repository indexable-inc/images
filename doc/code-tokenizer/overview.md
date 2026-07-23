# code-tokenizer

`packages/code-tokenizer` is a [tantivy](https://github.com/quickwit-oss/tantivy)
tokenizer that splits identifiers the way a code reviewer reads them: on
`camelCase`, `snake_case`, `kebab-case`, and any non-alphanumeric run. It is a
single Rust workspace library crate (`id = code-tokenizer`, no flake output)
consumed by the search index (`packages/file-search/Cargo.toml`).

## Public surface (`src/lib.rs`)

- `register_tokenizers(&tantivy::Index)` (`lib.rs:18-31`): registers both
  analyzers on an index's tokenizer manager.
- `CODE_TOKENIZER` = `"code"` (`lib.rs:15`): the raw split, for facet-style
  exact lookup.
- `CODE_STEMMED_TOKENIZER` = `"code_stemmed"` (`lib.rs:16`): the same split plus
  `LowerCaser` and an English `Stemmer`, for ranked full-text search
  (`lib.rs:24-30`).
- `CodeTokenizer` / `CodeTokenStream`: the `Tokenizer` / `TokenStream` impls if
  a caller wants to wire them manually.

## Internals

`CodeTokenizer::token_stream` first strips ANSI escape sequences from the input
(`strip_ansi_escapes`, `lib.rs:39-44`), so a terminal capture tokenizes the same
as its decoded text. The token stream (`advance`, `lib.rs:70-153`) emits one
token per identifier word, lowercasing each character, and breaks on:

- a separator (`_` or `-`, `is_separator`, `lib.rs:65-67`), which is consumed and
  not included in either token;
- a lower/digit -> upper transition (`fooBar` -> `foo`, `bar`);
- the end of an uppercase acronym run when the next char is lowercase
  (`HTTPServer` -> `http`, `server`; `getXValue` -> `get`, `x`, `value`,
  `lib.rs:119-128`);
- any non-alphanumeric character.

The acronym handling uses one-character lookahead so it can break *before* the
final uppercase that starts the next camel word. No flake package, no CLI:
library only.
