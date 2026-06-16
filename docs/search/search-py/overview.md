# search-py

`packages/search-py` is the PyO3 binding for
[`search-core`](../search-core/overview.md), imported as `search`. A read-only
query surface over the shared corpus store the
[`indexer`](../indexer/overview.md) populates: importing `search` and querying
never uploads the local checkout (`src/lib.rs:1-13`). All query, dedup, and
filter logic lives in the Rust core; this crate converts at the boundary. Crate
`search_py`, `crate-type = ["cdylib"]` (`Cargo.toml:8-10`).

## Entry points

Three thin async functions, each a `#[pyfunction]` returning a native asyncio
coroutine bridged through `pyo3-async-runtimes` (so callers `await` on their own
loop, `src/lib.rs:15`):

- `semantic(query, ...)` (`src/lib.rs:125`): natural-language search.
- `grep(pattern, ...)` (`src/lib.rs:214`): regex over the same chunks.
- `recent(...)` (`src/lib.rs:293`): newest records by descending timestamp, no
  semantic scoring (`compact` defaults `True` here).

The module is registered as `_search` (`src/lib.rs:493`); the Python package
`search/__init__.py` wraps each native coroutine with `_framed`
(`python/search/__init__.py:141`) so awaiting it yields a polars `DataFrame`
instead of a list of dicts.

Each Rust function resolves the store name (default `index`,
`DEFAULT_STORE`), the base URL (`mixedbread::DEFAULT_BASE_URL`), and builds the
scope filter with the shared `search_core::build_filter`, so the mapping matches
the [`search`](../search/overview.md) CLI exactly (`scope_filter`,
`src/lib.rs:338`). Each connects with `MixedbreadStore::from_login`
(`MXBAI_API_KEY`, else the `mgrep login` token) and runs with an empty manifest
and `CodeScope::ServerFiltered` (no checkout is read, `src/lib.rs:413`).

## Arguments and result frame

Keyword arguments mirror the CLI: `top_k`, `store`, `base_url`, `rerank` /
`reranker`, `web`, `agentic`, `compact`, and scope selectors `source` /
`not_source` / `repo` / `user` / `host` / `project` / `since` / `until`
(`src/lib.rs:98-120`). `since`/`until` accept an int (epoch seconds) or a string
(epoch or a relative span `"24h"`/`"7d"`, `TimeSpec`, `src/lib.rs:30`). An
unknown `source` raises `ValueError` listing the valid tags, since the store
silently returns zero hits for a typo (`parse_sources`, `src/lib.rs:365`).
`agentic` defaults `False` everywhere (slow and costly).

`hit_to_dict` (`src/lib.rs:465`) sets the six always-present keys (`path`,
`score`, `start_line`, `num_lines`, `text`, `source`) plus provenance keys only
when the record carries them. The Python wrapper enforces a stable column schema
on every frame (`_COLUMNS` / `_dtypes`, `__init__.py:79-115`), filling missing
provenance columns with nulls, so `df["timestamp"]` and `df.group_by("source")`
work even on an empty result.

```python
import search
df = await search.semantic("where is retry backoff configured")
df.group_by("source").len().sort("len", descending=True)
df = await search.recent(source=["shell"], user=["andrew"], since="6h")
```

## Build and distribution

`default.nix` packages the cdylib already built by the shared workspace unit
graph (`ix.rustWorkspace.units.libraries.search_py`) into the `ix-search` wheel
with `wheel/mkwheel.py`: no maturin, no second compile (`default.nix:10-13`). It
strips the build rpath and nixpkgs references and stamps manylinux tags. The
wheel is Linux-only (`package.nix:11`, `flake.systems = x86_64/aarch64-linux`):
the cdylib links on macOS too, but the wheel packaging strips an ELF rpath and
needs install-name fixups for macOS that no caller wants yet. `nix build
.#search-py` builds it.

For cross-platform `import search` (Linux and macOS), the `mcp` package bundles
this cdylib straight from the workspace graph into the `ix-mcp` interpreter, so
every MCP Python session can `import search` with no install step (README:126).
