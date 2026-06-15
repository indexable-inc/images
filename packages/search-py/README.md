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

df = await search.semantic("where is retry backoff configured")
df.select("path", "score", "timestamp").head()
df.group_by("source").len().sort("len", descending=True)

# only Claude history, only my records, last two weeks, token-frugal
df = await search.semantic(
    "deploy steps", source=["claude_history"], user=["andrew"],
    since="2w", compact=True,
)

# my shell commands of the last six hours, newest first
df = await search.recent(source=["shell"], user=["andrew"], since="6h")

df = await search.grep(r"fn \w+\(", source=["code"], repo="indexable-inc/index")
df.select("path", "text")
```

Each verb is an `async def` coroutine returning a [polars](https://pola.rs)
`DataFrame` (one row per hit), so `await` it on your own event loop and then
compose `.filter` / `.group_by` / `.sort` / `.head` on the result — the same
shape `fff` and `view` return.

## `semantic(query, ...)`

Keyword arguments mirror the `search` CLI:

- `top_k` (default `10`): maximum results.
- `store`: store name (default `index`).
- `base_url`: Mixedbread API base URL (default the client's built-in).
- `rerank` (default `True`): apply the second-stage reranker.
- `reranker` (default the listwise model): reranking model name; ignored when `rerank=False`.
- `web` (default `False`): mix in web results.
- `agentic` (default `False` on every surface): let the backend plan and run
  multiple searches. Measured 10-23s per query (vs 3-6s reranked single-shot)
  at ~5x the per-query price, and it may return fewer than `top_k` hits (it
  gates on its own judged relevance, on a different score scale than the
  reranker). Opt in only when recall matters more than latency.
- `compact` (default `False`): collapse repeated chunks of one document
  (keeping the best-scoring; the list refills from the overfetch buffer) and
  cap each snippet at 400 characters. A default `top_k=10` full response
  measured ~20k tokens; compact is roughly an order of magnitude smaller.
- scope: `source`, `not_source`, `repo`, `user`, `host`, `project`,
  `since`, `until` (see below).

## `grep(pattern, ...)`

Runs a regular expression over the same corpus chunks `semantic` covers:

- `top_k` (default `10`): maximum results.
- `store`, `base_url`: as above.
- `case_sensitive` (default `False`): match the pattern case-sensitively.
- `compact` (default `False`): as above.
- scope: `source`, `not_source`, `repo`, `user`, `host`, `project`,
  `since`, `until`.

## `recent(...)`

Lists the newest corpus records (descending `timestamp`) matching the scope —
a deterministic recency feed backed by the store's metadata-only chunk listing
(`/v1/stores/list-chunks`). No semantic scoring or reranking happens, so it is
fast; the `score` value in each hit is the API's placeholder, not relevance.

- `top_k` (default `20`): maximum records.
- `compact` (default `True` — a feed is scanned, not read; pass
  `compact=False` for full text).
- scope: `source`, `not_source`, `repo`, `user`, `host`, `project`,
  `since`, `until`.

```python
df = await search.recent(source=["shell"], since="6h")
df.select("timestamp", "text")
```

## Scope selectors

All optional; with none set the whole corpus is searched. List selectors accept
repeated values and comma-joined strings (`source=["code", "slack,linear"]`):

- `source` / `not_source`: include / exclude these source tags
  (`claude_history`, `codex`, `shell`, `claude_debug`, `git`, `github`,
  `slack`, `linear`, `code`, `web`). An unknown tag raises `ValueError`
  listing the valid tags — the store silently returns zero hits for a typo,
  which is indistinguishable from an empty corpus.
- `repo`: restrict code to a repository slug, e.g. `indexable-inc/index`.
- `user`, `host`, `project`: restrict records to these authors, machines, or
  project slugs.
- `since` / `until`: inclusive bounds on the record's epoch-second
  `timestamp`. Each accepts an `int` (epoch seconds) or a `str` holding epoch
  seconds or a relative span: `"90s"`, `"30m"`, `"24h"`, `"7d"`, `"2w"`.

## Result frame

Every verb returns a polars `DataFrame` with one row per hit and a stable
column schema, so `df["timestamp"]` and `df.group_by("source")` work even when
the result is empty or a provenance field is absent from every row (missing
values are null). The six always-present columns are `path`, `score`,
`start_line`, `num_lines`, `text`, and `source`; the provenance columns are set
only when the record carries them:

- `timestamp`: epoch seconds (the recency axis; every history record has one).
- `user`, `host`: who recorded it, where.
- `session_id`: Claude Code / codex / shell session — the handle for pulling
  the surrounding conversation.
- `external_id`: the record's stable id (e.g. `claude:{session}:{uuid}`).
- `url`: canonical web URL (GitHub items, Linear issues).
- `repo`: repository slug for code and git-commit hits.
- `project`: working-directory slug for agent-history hits.

Authentication mirrors the CLI: `MXBAI_API_KEY`, or the token written by
`mgrep login`.

## Distribution

Built by Nix, not a PEP 517 backend. `nix build .#search-py` compiles
the cdylib through the shared cargo-unit workspace graph and packages it as the
`ix-search` wheel (Linux-only manylinux tags).

It is also bundled into the [`ix-mcp`](../mcp) interpreter straight from the
workspace graph, so every MCP Python session can `import search` with
no install step on both Linux and macOS.
