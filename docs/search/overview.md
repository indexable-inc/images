# search

`packages/search` is the read-only semantic and regex search CLI over the shared
corpus store: one Mixedbread store holding code plus agent/shell history across
the fleet. It is a thin wrapper over [`search-core`](../search-core/overview.md)
(`packages/search/Cargo.toml:6`), binary `search`
(`nix run .#search`, `default.nix`).

`search` never indexes (`src/main.rs:1-8`). The separate
[`indexer`](../indexer/overview.md) owns all ingestion; this CLI queries the
store and projects hits. Querying never reads or uploads the local checkout, so
running it from any directory is safe (code is scoped entirely server-side via a
metadata filter, `src/main.rs:118`).

## Commands

A bare invocation runs a natural-language semantic search; subcommands cover the
rest (`Cli`/`Command`, `src/main.rs:36-68`):

- `search <pattern> [path]` - semantic search. Calls `search_core::semantic`
  (`src/main.rs:639`), or `search_core::ask` with `-a/--answer`
  (`src/main.rs:623`), or `search_core::ranked` when `--enhance` derives a sort
  (`src/main.rs:604`).
- `search grep <pattern>` - regex over the same indexed chunks
  (`search_core::grep`, `src/main.rs:1035`); local-corpus only (no web).
- `search recent` - newest records by descending `timestamp`, no semantic
  scoring (`search_core::recent`, `src/main.rs:675`).
- `search context <id>` - expand a hit's `external_id` (or a bare session id)
  into the surrounding conversation (`search_core::context`, `src/main.rs:698`).
- `search stats` - per-source census: document counts and freshness
  (`search_core::stats`, `src/main.rs:723`).

## Flags

Key semantic-path flags (`SemanticArgs`, `src/main.rs:206`): `-m/--max-count`,
`-c/--content`, `-a/--answer` (+ `--instructions`, `--no-cite`,
`--no-multimodal`), `--no-rerank` / `--reranker <model>` / `--rerank-top-k`,
`-w/--web`, `--agentic` (+ `--agentic-max-rounds`, `--agentic-instructions`),
`--rewrite-query`, `--no-search-rules`, `--enhance`, `--json`, `--compact`.
`--agentic` is off by default on every surface: it costs 10-23s per query (vs
3-6s reranked) at ~5x the price and may return fewer than `--max-count` hits
(`src/main.rs:265`).

Scope selectors are shared by the semantic, grep, recent, and stats paths
(`ScopeArgs`, `src/main.rs:73`): `--source` / `--not-source`, `--repo`,
`--user` / `--mine` (the current `$USER`), `--host`, `--project`,
`--since` / `--until`. An unknown `--source` is a hard error: the store
silently returns zero hits for a typo, indistinguishable from an empty corpus
(`parse_sources`, `src/main.rs:169`, validated against `KNOWN_SOURCE_TAGS`).
`resolve_scope` (`src/main.rs:121`) maps them into a `search_core::FilterSpec`
and `build_filter`, the same builder the Python binding uses.

Connection flags on every subcommand: `--store` / `MXBAI_STORE` (default
`index`), `--base-url` / `MXBAI_BASE_URL`. `connect` (`src/main.rs:504`)
authenticates with `MixedbreadStore::from_login` (`MXBAI_API_KEY`, else the
`mgrep login` token; README:18-24).

## Pipe-in rerank mode

Piped stdin switches behavior: `ls | search "query"` ranks the piped lines
against the query with the reranking model instead of searching the corpus
(`src/main.rs:10`, `run`/`run_piped` at `src/main.rs:538`/`1100`,
`piped_stdin_lines` at `src/main.rs:1058`). A TTY, `</dev/null`, or an empty
pipe falls through to the normal corpus search. Pipe mode rejects flags that
make no sense for it (`--answer`, `--web`, `--agentic`, `--enhance`, scope
selectors, a path arg) with explicit errors, and uses `mixedbread`'s `rerank`
endpoint directly.

## Output

Human listing by default with TTY-gated color (`NO_COLOR` / `CLICOLOR_FORCE`
honored, syntax highlight via `code-highlight`); `--json` emits the stable
`search_core::hits_to_json` array (`src/main.rs:1305`); `--compact` collapses
repeated chunks and caps snippets. `--answer` prints the synthesized answer with
`[n]` citations over a numbered source list (`print_answer`, `src/main.rs:513`).

## Build

`default.nix` uses `ix.cargoUnit.selectBinaryWithTests` (binary `search`,
`mainProgram` `search`); `package.nix` sets `flake = true` and `packageSet =
true`, so it is `nix run .#search` and `pkgs.search`. Dependencies are
`search-core`, `mixedbread`, `code-highlight`, `terminal-theme`,
`progress-style`, clap, indicatif (`Cargo.toml:15-31`).
