# file-search

`packages/file-search` is a BM25 file indexer and searcher built on
[Tantivy](https://github.com/quickwit-oss/tantivy) (`Cargo.toml:6`). It is a
purely local full-text tool: it builds an on-disk index over a directory tree
and answers keyword queries. It does not touch the shared corpus store, the
Mixedbread client, or any source adapter; it is the lexical counterpart to the
semantic [`search`](../search/overview.md) stack. Library crate `file_search`
plus a `file-search` binary (`Cargo.toml:11-17`), `nix build .#file-search`.

## Public surface (`src/lib.rs`)

- [`SearchIndexReader`] (`src/lib.rs:35`): read-only view over an on-disk index.
  `open(index_dir)` (`src/lib.rs:50`) registers the code tokenizers and builds a
  reader without taking the writer lock, so many readers run concurrently with at
  most one writer. `search(query, limit, filter_directory)` (`src/lib.rs:76`)
  returns the top hits; `filter_directory` restricts to a directory and its
  descendants.
- [`SearchIndex`] (`src/lib.rs:96`): read-write handle. `open_or_create`
  (`src/lib.rs:112`) opens or creates the index (deriving the schema from the
  on-disk index, not the freshly built one, so an older field order cannot
  corrupt reads), `index_directory(dir, respect_gitignore)` (`src/lib.rs:156`)
  walks and indexes, `search(...)` mirrors the reader.
- [`EphemeralSearch`] (`src/ephemeral.rs:24`): an in-memory BM25 reranker over a
  `RamDirectory`. `from_texts(texts)` builds a one-shot index; `search(query,
  limit)` returns `RankResult { id, score }` where `id` is the text's position in
  the input iterator. For callers that just want to rerank a batch in memory.
- Re-exports the `repo_walker` walk API and `types::{IndexStats, SearchResult}`
  (`src/lib.rs:20-23`).

## Indexing (`src/indexing.rs`)

`index_directory` (`src/indexing.rs:79`) first wipes every doc whose `directory`
lives under the indexed root (a Tantivy `RangeQuery` over the trailing-slash
`[<root>/, <root>0)` encoding, `src/indexing.rs:91`), so files deleted, renamed,
or newly gitignored between runs disappear; a walk failure rolls the wipe back so
a transient error never blows away the previous index (`src/indexing.rs:109`).
Per-file: files over `MAX_FILE_SIZE` (1 MiB) are skipped, paths are canonicalized
once so a re-index with an equivalent spelling lines up with the prior
`path_exact` term, and the file is split into 500-char chunks with 100-char
overlap (`chunk_content`, `src/indexing.rs:33`), each indexed as its own document
with a `chunk_offset`. The delete-then-add keys on the untokenized `path_exact`
term (`src/indexing.rs:196`); deleting via the stemmed `path` field would no-op.
Per-file read/parse errors are recorded in `IndexStats.errors`, not fatal.

## Schema (`src/schema.rs`)

`build_schema` (`src/schema.rs:4`) defines fields: `path` and `content` and
`filename` (tokenized with the `CODE_STEMMED_TOKENIZER` from the
`code-tokenizer` crate, stored, with freqs+positions), `path_exact` (untokenized
keyword, for exact-match deletes), `chunk_offset` (stored u64), and `directory`
and `extension` (untokenized keyword strings, so byte-range filters match an
exact dir plus descendants without catching same-prefix siblings like
`src-old`).

## CLI (`src/main.rs`)

Two subcommands (`src/main.rs:18-43`): `index <directory> [--no-gitignore]`
opens-or-creates the index and walks the tree (printing indexed/skipped/error
counts); `search <query> [--limit N] [--filter PATH]` opens a read-only reader
and prints `score path[ @offset]`. Search avoids the writer so it can run against
a shared or read-only index concurrently with an indexing run (`src/main.rs:83`).
The index dir comes from `--index-dir`, else `FILE_SEARCH_INDEX_DIR`, else
`<cache>/file-search/index` (`src/main.rs:113`). Query syntax is Tantivy's.

## Build

`default.nix` selects the `file-search` binary from the workspace graph;
`package.nix` sets `flake`/`packageSet`, so `nix build .#file-search` and
`pkgs.file-search`. Deps: tantivy, `code-tokenizer`, `repo-walker`, clap, snafu
(`Cargo.toml:19-24`); a tango-bench benchmark lives at `benches/search_bench.rs`.

[`SearchIndexReader`]: #public-surface-srclibrs
[`SearchIndex`]: #public-surface-srclibrs
[`EphemeralSearch`]: #public-surface-srclibrs
