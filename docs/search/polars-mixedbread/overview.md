# polars-mixedbread

`packages/polars-mixedbread` is a [Polars](https://pola.rs) IO source backed by
[Mixedbread](https://www.mixedbread.com) store search. `scan_mixedbread(query,
store=...)` returns a lazy `pl.LazyFrame` whose rows are the hits of one search,
with file metadata flattened into typed columns, so you can do in Polars
everything Mixedbread does not, above all `group_by` (README:1-6). PyO3 cdylib
plus a Python wrapper; `nix build .#polars-mixedbread`.

## Split: thin Rust, logic in Python

The Rust crate is deliberately thin (`src/lib.rs:1-15`): `search_mixedbread`
(`src/lib.rs:54`) reuses the workspace [`mixedbread`](../mixedbread/overview.md)
client (HTTP, retry, auth, the filter DSL), blocks on one async search, and
converts the `mixedbread::Chunk`s into a dict of column-name to list, always six
columns: `text`, `score`, `filename`, `start_line`, `num_lines`, `metadata` (the
raw JSON string). `filters` arrives as a JSON string (the recursive filter
shape) and is pushed server-side. Auth mirrors the `search` surface
(`MXBAI_API_KEY`, else the `mgrep login` token). Polars never appears on the Rust
side, so there is no Rust/Python Polars version coupling (unlike
[`polars-sftp`](../polars-sftp/overview.md), which decodes in Rust)
(`src/lib.rs:12`).

Everything else lives in the Python wrapper
(`python/polars_mixedbread/__init__.py`), where the runtime Polars is:
`scan_mixedbread` (`__init__.py:110`) registers an IO source via
`register_io_source`, flattens declared metadata keys out of the JSON `metadata`
column into typed columns, applies the predicate, and applies projection and the
row limit.

## Predicate pushdown

One unified API: you filter with ordinary Polars expressions, and the source
parses the predicate and pushes the parts that map to a Mixedbread metadata
filter (string `==`/`!=` on a metadata column, combined with `&`/`|`/`~`)
server-side (`_pushdown.pushdown`); anything else (a `score` threshold, `is_in`,
substring) runs client-side. The full predicate is always re-applied locally, so
every returned row satisfies it (`__init__.py:182-186`).

Pushdown is not transparent, and that is the point of a search source: a pushed
filter is applied by Mixedbread BEFORE ranking and `top_k`, so you get the
`top_k` best hits within the filter; the same predicate written so it cannot push
(`is_in(["code"])` vs `== "code"`) filters after ranking and can return a
different set (both correct, README:30-38). Only string columns push down (string
equality is unambiguous); a non-string declared column still filters and groups,
just in Polars.

## top_k, min_results, depth

`top_k` is retrieval depth (how many ranked hits come back), not a final row
count. A server-pushed filter applies before `top_k` (up to `top_k` of the
filtered set); a client-side filter applies after (can leave fewer). For a final
cap use Polars' `.head(n)`. For an output floor, pass `min_results=N`: a
client-side filter that trims below N triggers a re-search with a growing `top_k`
(doubling, `_overfetch.grow_until`) until N rows survive or the store is
exhausted; `max_top_k` is the hard ceiling (README:47-75). `scan_mixedbread`
parameters (`__init__.py:110`): `query`, `store` (name or list, default `index`),
`top_k`, `min_results`, `max_top_k`, `base_url`, `rerank` / `reranker`,
`agentic`, `score_threshold`, `metadata_columns`.

## Columns

`_INTRINSIC` (the six always-present, `__init__.py:91`) plus one typed column per
`metadata_columns` entry; the default surfaces the `index` store's keys
(`source`, `repo`, `path`, `title`, `__init__.py:103`). Point it at another store
by passing that store's keys. The raw `metadata` column is always present, so
undeclared keys stay reachable via `pl.col("metadata").str.json_decode()`.

```python
from polars_mixedbread import scan_mixedbread
lf = scan_mixedbread("how does retry backoff work", store="index", top_k=500)
lf.filter(pl.col("source") == "code").group_by("repo").agg(pl.len()).collect()
```

Bad fit for a full table scan or store-wide aggregation: this is top-k
retrieval, so a `group_by` aggregates only the retrieved window
(README:108-113).

## Build

`package.nix` is `inRustWorkspace = true` (the cdylib is built by the shared
unit graph, reusing the `mixedbread` client) with `flake`/`packageSet`, so
`nix build .#polars-mixedbread`; `default.nix` only packages the wheel. It is
cross-platform (consumed inside the Nix env, e.g. the MCP Python session, not
redistributed, so darwin needs no install-name fixups, `package.nix:6-9`). The
pure-Python pushdown test surfaces as the CI check
`checks.<system>.polars-mixedbread-pushdown` (`package.nix:12-16`).
