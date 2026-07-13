# tree-sitter-nix (fork)

Fork of [nix-community/tree-sitter-nix] 0.3.0 (the crates-io release), carrying
one grammar change: underscore digit separators in numeric literals, matching
the `nix-ix` lexer patch
(`packages/nix/nix/patches/0014-libexpr-accept-underscore-digit-separators-in-numeri.patch`).
One or more underscores may appear between any two digits of the integer part,
the fractional part, and the exponent (`1_000`, `1_000_000.000_1`, `2.5e1_0`);
a leading underscore still starts an identifier, so `_1_000` stays a
`variable_expression`.

The root `Cargo.toml` substitutes this crate for the crates-io release via
`[patch.crates-io]`, so every workspace consumer of `ast-merge-langs` (astlog,
clone-detect, ...) parses the dialect the patched nix accepts.

## Layout

Restructured from the upstream crate to the workspace shape (the rust
workspace source fileset ships `Cargo.toml`, `build.rs`, and `src/` only):
`bindings/rust/lib.rs` became `src/lib.rs`, and the queries `src/lib.rs`
embeds moved under `src/queries/`. `src/parser.c` and `src/node-types.json`
are generated artifacts, committed like every tree-sitter grammar ships them.

## Regenerating

After editing `grammar.js`:

```sh
tree-sitter generate   # writes src/parser.c, src/node-types.json, ...
```

(`tree-sitter-cli`; ABI 14.) `src/scanner.c` is hand-written upstream code and
is not regenerated. Drop the generated `src/grammar.json` — only `parser.c`
and `node-types.json` are consumed here.

[nix-community/tree-sitter-nix]: https://github.com/nix-community/tree-sitter-nix
