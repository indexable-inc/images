# Glossary: disambiguating overloaded names

Several names in this repo mean more than one thing. `search` is a CLI, a Rust
lib, a Python import, and a wheel with a different name again; `index` is a repo,
a store, a verb, and a package. This page lists every real meaning of each
overloaded term with its source, so an agent never conflates them. When in
doubt, match the meaning to the entry point you are about to invoke (`nix run
.#<name>`, an `import`, or an `ix` subcommand) and check it here first. See also
[cli.md](cli.md), [overview.md](overview.md), and [environment.md](environment.md).

## search

| Meaning | What it is | Source |
| --- | --- | --- |
| `.#search` | Read-only semantic + regex CLI over the shared corpus store. Never indexes locally. Needs `MXBAI_API_KEY` (or a stored token) or it fails at auth. | `packages/search/Cargo.toml:2`, `packages/search/src/main.rs:3` |
| `search-core` | Rust library the CLI is a thin wrapper over. | `packages/search-core/Cargo.toml:2` |
| `search-py` | PyO3 bindings. Imported in Python as `import search`; the wheel is named `ix-search`. | `packages/search-py/python/search/__init__.py`, `packages/search-py/pyproject.toml:9` |
| `file-search` | Independent BM25 / Tantivy file-and-path indexer. Unrelated to the semantic store. `.#file-search`. | `packages/file-search/Cargo.toml:2,6` |
| `polars-mixedbread` | Polars IO plugin; `import polars_mixedbread`. | `packages/polars-mixedbread/pyproject.toml:6` |
| `mixedbread` | Rust client lib for the Mixedbread vector store. | `packages/mixedbread/Cargo.toml:2` |
| `search-eval` | Search-quality evaluation harness. `.#search-eval`. | `packages/search-eval/pyproject.toml:2` |
| `indexer` | The thing that actually ingests content into the store; `search` only queries what `indexer` populates. | `packages/indexer/Cargo.toml:2,6` |

The default Mixedbread store is itself named `index`
(`packages/search/src/main.rs:1569`).

## index

- The **repo**: `indexable-inc/index`.
- The **default store name** that `search` queries and `indexer` fills
  (`packages/search/src/main.rs:1569`).
- The **verb** "to index" (ingest content) - owned by `indexer`, not by
  `search` (`packages/search/src/main.rs:3`).
- The **package** `indexer` that performs that verb
  (`packages/indexer/Cargo.toml:2`).

## run

Three unrelated things:

- **`nix run`** - the Nix command that launches any flake app (`.#search`,
  `.#mcp-ex`, ...).
- **`.#run`** - terminal session recorder: records a command's terminal
  session, timing, and queryable output events. `nix run .#run -- <cmd>`
  (`packages/tui/run/default.nix:14,17`).
- **`ix run`** - boots a fresh VM, runs one command, streams output, leaves the
  VM up (`doc/ix/cli.md:18-19,35`).

## mcp

- The **protocol** (Model Context Protocol).
- The **server** package: flake app `.#mcp-ex`, the Elixir kernel, binary
  `ix-mcp-ex` (`packages/mcp-ex/`, `packages/mcp-ex/README.md`). This is the
  only MCP kernel; the retired Python server (`packages/mcp`, `ix_notebook_mcp`)
  is gone.

## fleet

- **`.#ix-fleet`** - the CLI/tool that renders and executes fleet plans
  (`packages/ix-fleet/Cargo.toml:2,4`).
- A **fleet** - the set of remote ix VMs a plan describes. Off the product
  surface since indexable-inc/ix#8306: an ix project defines one VM.
- A node in that fleet is called a **branch** by the ix SDK (`ix_sdk.BranchStatus`)
  (`packages/ix-fleet/src/ix_fleet/__init__.py:221`).

## dashboard

- **`.#dashboard`** - standalone web canvas that aggregates every ix resource
  producer socket into one live board (`packages/dashboard/dashboard/Cargo.toml:2,6`).
- **`dashboard-core`** - the shared library: wire types, pane publisher, canvas
  server (`packages/dashboard/dashboard-core/Cargo.toml:2,6`).

## Wheel / import / binary names

These do not match each other - check this table before importing or invoking.

| Package | Flake app | Binary | Python import | Wheel / dist name |
| --- | --- | --- | --- | --- |
| `search` | `.#search` | `search` | - | - |
| `search-py` | - | - | `search` | `ix-search` |
| `polars-mixedbread` | - | - | `polars_mixedbread` | `polars-mixedbread` |
| `file-search` | `.#file-search` | `file-search` | - | - |

## Which entry point should I use?

- **Semantic search** of indexed content -> `.#search` (CLI) or `import search`
  (Python). Set `MXBAI_API_KEY` first.
- **File / path search** (BM25, local) -> `.#file-search`.
- **Record a terminal run** -> `.#run`.
- **Ingest / index content** into the store -> `.#indexer`.

## room-server

`room-server`: a Rust backend in the private ix monorepo (`crates/room/server`)
that runs agent turns for the Weave runtime (which superseded the decommissioned
Symphony, ENG-7401). It is not built in this public repo; references in
`doc/codex` resolve to it.

## See also

- [overview.md](overview.md): what is open vs hosted, and the page map.
- [cli.md](cli.md): the `ix` subcommands these names can collide with.
- [environment.md](environment.md): the `IX_*` variables referenced throughout.
