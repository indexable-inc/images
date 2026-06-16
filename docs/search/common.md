# Search

Semantic and full-text code search for index, plus the corpus pipeline that
feeds it. One shared vector store (Mixedbread) holds the whole fleet's corpus:
code from every checkout, plus agent and shell history (Claude Code, Codex,
atuin), bulk exports (Slack, Linear, GitHub, git commits), and journald logs.
Source adapters turn each of those into embeddable [`Document`]s, the `indexer`
syncs them into the store (and a durable parquet/Iceberg log), and the `search`
CLI, the `search` Python binding, and `search-eval` query and grade them.
Two local-only full-text tools (`file-search`, `fff`) round out the domain but
do not touch the corpus store.

Read this page first, then the component page for the unit you are touching.
The load-bearing concepts (the [`Document`] envelope, content addressing, the
[`Store`]/`Reconciler` contracts) live in [search-core](search-core/overview.md)
and [source](source/overview.md).

## Units

Everything except `fff` is a Rust workspace member (root `Cargo.toml`). `fff`
is a pinned third-party Rust toolkit built outside the workspace. Library
crates (`search-core`, `source/*`, `sink/*`, `lake-iceberg`, `mixedbread`) have
no flake output; the binaries and wheels do (`nix run`/`nix build .#<id>`).

| unit | role |
| --- | --- |
| `packages/search-core` | content-addressed semantic+regex search core: manifest, dedup sync, the [`Store`] backend trait, the query/projection/filter logic. Library. See [search-core](search-core/overview.md). |
| `packages/search` | read-only semantic + regex search CLI over the shared store (`nix run .#search`). Thin wrapper over `search-core`. See [search](search/overview.md). |
| `packages/search-py` | PyO3 binding (`import search`): async `semantic`/`grep`/`recent` returning polars frames (`nix build .#search-py`). See [search-py](search-py/overview.md). |
| `packages/search-eval` | Exa-style retrieval+agentic quality harness for `search` (Python, `nix run .#search-eval`). See [search-eval](search-eval/overview.md). |
| `packages/indexer` | drives every source adapter and fans documents out to Mixedbread + S3/R2 parquet + the Iceberg lake (`nix run .#indexer`). See [indexer](indexer/overview.md). |
| `packages/source` (workspace: meta, atuin, claude, codex, debug, git, github, journald, linear, parquet, slack) | source adapters turning each data source into tagged `Document`s, behind the shared `source-meta` envelope/trait. See [source](source/overview.md). |
| `packages/sink` (workspace: mixedbread, parquet) | reconcile a source's documents into a view: a Mixedbread store, or an S3/R2 parquet corpus log. See [sink](sink/overview.md). |
| `packages/lake` (iceberg) | the Iceberg corpus lake: append+tombstone document log with snapshot-cursor reads. Library. See [lake](lake/overview.md). |
| `packages/mixedbread` | minimal async Rust client for the Mixedbread vector store API. Library. See [mixedbread](mixedbread/overview.md). |
| `packages/file-search` | BM25 file indexer/searcher on Tantivy; local, no corpus store (`nix build .#file-search`). See [file-search](file-search/overview.md). |
| `packages/fff` | fast file-search toolkit (`fff-mcp` CLI/MCP server + `fff-c` cdylib), third-party (`nix run .#fff`). See [fff](fff/overview.md). |
| `packages/polars-mixedbread` | PyO3 `scan_mixedbread` polars IO source over a Mixedbread search (`nix build .#polars-mixedbread`). See [polars-mixedbread](polars-mixedbread/overview.md). |
| `packages/polars-sftp` | PyO3 `scan_sftp` polars IO source over SFTP (`nix build .#polars-sftp`). See [polars-sftp](polars-sftp/overview.md). |

## How it fits together

```
data source                 adapter (source/*)        sinks (indexer fan-out)        query (read)
checkout files          ->  search-core code sync  ->  Mixedbread store         <-  search CLI
~/.claude, ~/.codex     ->  SourceAdapter          ->  + sink-parquet (S3/R2)   <-  search-py (import search)
atuin db, journald      ->    -> [Document]         ->  + lake-iceberg          <-  scan_mixedbread
Slack/Linear/GitHub     ->                              (durable corpus log)        search-eval (grades search)
git history             ->                              \--> consume: log -> Mixedbread (rebuild/catch-up)
```

- **One Document model, one envelope.** Every record, whatever its source, is a
  [`source_meta::Document`] (`packages/source/meta/src/lib.rs:199`): an
  `external_id`, the UTF-8 `body` to embed, a flat `meta_json` (a common
  [`DocumentMeta`] header flattened to top-level keys, merged with
  source-specific extras), and a `content_hash`. The canonical metadata key
  names are `source_meta::keys` (`packages/source/meta/src/keys.rs`), shared by
  adapters (which write them) and the filter builder (which queries them).
- **Adapters produce desired state.** A [`SourceAdapter`]
  (`packages/source/meta/src/lib.rs:219`) turns one corpus into an iterator of
  `Document`s; a [`Reconciler`] (`...:251`) converges a view (a store, a
  parquet log, the lake) to that set. The contract: desired state not deltas,
  idempotent on `(external_id, content_hash)`, source-scoped.
- **The indexer is the only writer of the corpus.** It opens each selected
  source, makes one pass over its `documents()`, and fans the set out to every
  configured sink (`packages/indexer/src/main.rs:1593`). Code repos go straight
  to Mixedbread via `search-core`'s content-addressed sync; everything else can
  also land in the durable parquet/Iceberg log, which a separate consume run
  replays back into Mixedbread.
- **Reads go through `search-core`.** The `search` CLI, the `search` Python
  binding, `scan_mixedbread`, and `search-eval` all query the store the indexer
  populated; none of them index (code sync lives only in `search-core`'s
  pipeline, used by the indexer, not the read CLIs).

## Invariants

- **content_hash is the hash of the embedded bytes.** `source_meta::hash_body`
  (`packages/source/meta/src/lib.rs:271`) is the only constructor, formatted
  `sha256:<hex>`. It is the change-detection key everywhere: a re-sync of
  unchanged content is a no-op; a changed body re-embeds. For code it equals the
  manifest's content hash (`packages/search-core/src/content.rs`), which doubles
  as the store `external_id`, so byte-identical files across many worktrees share
  one stored embedding.
- **Code sync never deletes.** A stored code entry is shared across checkouts,
  so deletion can only be decided by reference counting across every manifest;
  it lives in a separate GC pass, never in ordinary sync
  (`packages/search-core/src/sync.rs:1`). Record sources delete only via an
  explicit `--gc` pass (`MixedbreadReconciler::gc`) or, in the lake, explicit
  tombstones; absence from a newer pass never deletes.
- **The store is shared; waits are scoped to the run.** Every embedding wait
  gates on exactly the ids this pass uploaded, never the store-wide pending
  counts, so another source's backlog cannot stall a run (ENG-2699,
  `packages/search-core/src/sync.rs:226`).
- **Scoped listings are verified.** A reconcile/GC pass that lists by
  `source == X` rechecks every returned record's own `source` before acting; a
  backend that drops the filter aborts loudly instead of feeding a store-wide
  delete set in (`packages/sink/mixedbread/src/lib.rs:92`).
- **source is the primary scope filter, an open set.** [`Source`] is a string
  tag, not an enum (`packages/source/meta/src/lib.rs:50`); adding a corpus is a
  new tag value. Query edges validate user-supplied tags against
  `KNOWN_SOURCE_TAGS` (`...:62`) because the store silently returns zero hits
  for a typo.
- **Code results are worktree-scoped by the manifest; record sources by the
  server filter.** Search over-fetches and keeps only code whose content hash is
  in the local manifest (`CodeScope::WorktreeExact`); record sources have no
  checkout, so their server-side metadata filter is authoritative
  (`packages/search-core/src/search.rs`).
- **Auth is uniform.** Every Mixedbread surface resolves `MXBAI_API_KEY`, else
  the OAuth token written by `mgrep login` (exchanged for a short-lived API JWT),
  via `mixedbread::auth` (`packages/mixedbread/src/auth.rs`).

## Glossary

- **Document**: one record ready to embed: `external_id`, `body`, flat
  `meta_json`, `content_hash` (`packages/source/meta/src/lib.rs:199`).
- **DocumentMeta**: the common header every record carries, flattened to
  top-level metadata keys (`source`, `external_id`, `content_hash`, `title`,
  `url`, `timestamp`).
- **content_hash / external_id**: the sha256 of the embedded body, and the
  store-stable per-record id. For code they are the same value.
- **source / source tag**: which corpus a record came from (`code`, `shell`,
  `slack`, ...); the primary scope filter, an open string set.
- **manifest**: a checkout's `relative path -> content hash` view, persisted in
  SQLite, used to scope code search and decide what to upload
  (`packages/search-core/src/manifest.rs`).
- **signature**: a content-addressed digest of a manifest's hash set; an
  unchanged signature lets sync skip the network entirely.
- **Store**: `search-core`'s backend trait (search/grep/upload/list/facets/...);
  `MixedbreadStore` is the production impl, `MemoryStore` the test one.
- **Reconciler**: a view that converges to a source's desired-state documents
  (Mixedbread, parquet, lake). reconcile keeps absences; replace deletes them.
- **tombstone**: an explicit lake `op = delete` row; the lake's only deletion
  path, never inferred from absence (ENG-2696).
- **slice / version**: the lake's writer identity (`host`, optional `user`) and
  per-slice revision counter that orders the append-only log.
- **overfetch / projection**: search asks for more than `top_k`, then projects
  raw chunks into display hits, dropping out-of-scope code and capping the list.
- **rerank / agentic / enhance**: second-stage listwise reranking (on by
  default), multi-round backend search (off by default, slow/costly), and
  server-side query-to-filter rewriting.
- **corpus log / lake**: the durable, replayable source of truth (S3/R2 parquet
  or Iceberg) under the Mixedbread index, which is a materialized view of it.

## Components

| component | page | what |
| --- | --- | --- |
| search-core | [search-core/overview.md](search-core/overview.md) | content-addressed core: manifest, dedup sync, Store trait, query/projection/filter |
| search | [search/overview.md](search/overview.md) | read-only semantic + regex search CLI + pipe-rerank mode |
| search-py | [search-py/overview.md](search-py/overview.md) | `import search`: async semantic/grep/recent polars frames |
| search-eval | [search-eval/overview.md](search-eval/overview.md) | Exa-style retrieval + agentic quality harness |
| indexer | [indexer/overview.md](indexer/overview.md) | sync every source into Mixedbread + parquet + the lake |
| source | [source/overview.md](source/overview.md) | the source-meta envelope/trait and the adapters ([adapters.md](source/adapters.md)) |
| sink | [sink/overview.md](sink/overview.md) | Mixedbread reconcile/replace/GC and the parquet corpus log |
| lake | [lake/overview.md](lake/overview.md) | the Iceberg append+tombstone corpus log with cursor reads |
| mixedbread | [mixedbread/overview.md](mixedbread/overview.md) | minimal async Rust client for the Mixedbread API |
| file-search | [file-search/overview.md](file-search/overview.md) | local BM25 file indexer/searcher on Tantivy |
| fff | [fff/overview.md](fff/overview.md) | third-party fast file-search toolkit (fff-mcp + fff-c) |
| polars-mixedbread | [polars-mixedbread/overview.md](polars-mixedbread/overview.md) | `scan_mixedbread` polars IO source |
| polars-sftp | [polars-sftp/overview.md](polars-sftp/overview.md) | `scan_sftp` polars IO source over SFTP |

[`Document`]: source/overview.md
[`source_meta::Document`]: source/overview.md
[`DocumentMeta`]: source/overview.md
[`SourceAdapter`]: source/overview.md
[`Reconciler`]: source/overview.md
[`Source`]: source/overview.md
[`Store`]: search-core/overview.md
