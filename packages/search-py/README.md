# search-py

PyO3 bindings for [`search-core`](../search-core). Imported as `search`.

A read-only query surface over the shared `index` corpus the
[`indexer`](../indexer) populates (code plus agent/shell history across the
fleet). This binding never indexes, so importing `search` and querying never
uploads your local checkout. All query, dedup, and filter logic lives in the
Rust core crate; this package is a thin binding that converts results at the
boundary.

```python
import search

hits = await search.semantic("where is retry backoff configured")
for hit in hits:
    print(hit["path"], hit["score"])

# only Claude history, only my records
hits = await search.semantic("deploy steps", source=["claude_history"], user=["andrew"])

hits = await search.grep(r"fn \w+\(", source=["code"], repo="indexable-inc/index")
for hit in hits:
    print(hit["path"], hit["text"])
```

Each verb returns a native asyncio coroutine, so `await` it on your own event
loop.

## `semantic(query, ...)`

Keyword arguments mirror the `search` CLI:

- `top_k` (default `10`): maximum results.
- `store`: store name (default `index`).
- `base_url`: Mixedbread API base URL (default the client's built-in).
- `rerank` (default `True`): apply the second-stage reranker.
- `web` (default `False`): mix in web results.
- scope: `source`, `not_source`, `repo`, `user`, `host`, `project` (see below).

## `grep(pattern, ...)`

Runs a regular expression over the same corpus chunks `semantic` covers:

- `top_k` (default `10`): maximum results.
- `store`: store name (default `index`).
- `base_url`: Mixedbread API base URL (default the client's built-in).
- `case_sensitive` (default `False`): match the pattern case-sensitively.
- scope: `source`, `not_source`, `repo`, `user`, `host`, `project`.

## Scope selectors

All optional; with none set the whole corpus is searched. List selectors accept
repeated values and comma-joined strings (`source=["code", "slack,linear"]`):

- `source` / `not_source`: include / exclude these source tags (`code`,
  `claude_history`, `codex`, `shell`, `slack`, `linear`, `web`).
- `repo`: restrict code to a repository slug, e.g. `indexable-inc/index`.
- `user`, `host`, `project`: restrict records to these authors, machines, or
  project slugs.

Each hit is a dict with `path`, `score`, `start_line`, `num_lines`, `text`, and
`source`. Authentication mirrors the CLI: `MXBAI_API_KEY`, or the token written
by `mgrep login`.

## Distribution

Built by Nix, not a PEP 517 backend. `nix build .#search-py` compiles
the cdylib through the shared cargo-unit workspace graph and packages it as the
`ix-search` wheel (Linux-only manylinux tags).

It is also bundled into the [`ix-mcp`](../mcp) interpreter straight from the
workspace graph, so every MCP Python session can `import search` with
no install step on both Linux and macOS.
