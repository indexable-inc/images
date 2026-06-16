# search-core

`packages/search-core` is the content-addressed semantic code search core:
content hashing, the local manifest, dedup-aware sync, the backend `Store`
trait, and the query/projection/filter logic. It is a library (crate
`search_core`, `packages/search-core/Cargo.toml:12`); the [`search`](../search/overview.md)
CLI, the [`search-py`](../search-py/overview.md) binding, and the
[`indexer`](../indexer/overview.md) all depend on it. No flake output; it is a
workspace unit.

The design in one paragraph (`src/lib.rs:1-16`): files are identified by the
hash of their bytes, not their path, so byte-identical files across many
worktrees, branches, or repos share a single stored embedding. Sync uploads
only content the store is missing and never deletes. A local [`Manifest`]
records this checkout's `path -> hash` view; search over-fetches from the shared
store and keeps only hits whose hash is in the manifest, so results reflect the
current tree. There is no daemon: each run rebuilds the manifest cheaply (mtime
skips re-hashing) and uploads what is new.

## Modules (`src/lib.rs:23-35`)

- **`content`** - [`ContentHash`], `sha256:<hex>` over a file's bytes; doubles
  as the Mixedbread `external_id`, which is what makes the store dedup
  (`src/content.rs:12`).
- **`manifest`** - [`Manifest`]/[`FileEntry`]: build by walking a root, persist,
  and the content [`signature`](#manifest-and-signature). See below.
- **`db`** - SQLite persistence of every checkout's manifest, keyed by
  `(root, rel_path)`, plus the per-`(root, store)` synced signature
  (`src/db.rs`).
- **`backend`** - the [`Store`] trait and `MemoryStore` test impl, plus the
  query option/hit/provenance types (`src/backend.rs`).
- **`adapter`** - [`MixedbreadStore`], the production `Store` over the
  [`mixedbread`](../mixedbread/overview.md) client (`src/adapter.rs`).
- **`sync`** - dedup-aware upload and `wait_until_indexed` (`src/sync.rs`).
- **`pipeline`** - `index_and_{semantic,grep,answer}`: build the manifest, embed
  new files, then query, in one call (`src/pipeline.rs`).
- **`search`, `context`, `query_filter`, `repo`, `config`** - the read/query
  layer (projection, conversation windows, the metadata filter builder, repo
  slug derivation, runtime limits). See [internals](internals.md).

## Content addressing and dedup

[`ContentHash::of_bytes`] (`src/content.rs:23`) formats `sha256:<hex>`. Two
uploads with the same content produce the same `external_id`, so the second is a
store no-op the sync skips. The crate re-exports `source_meta::Document`,
`Source`, `RepoSlug`, and `hash_body` (`src/lib.rs:64`), so the content hash a
code file gets here is the same `content_hash` every other source uses (see
[source](../source/overview.md)).

## Manifest and signature

[`Manifest::build`] (`src/manifest.rs:66`) walks `root` with the `repo-walker`
crate (a separate package), honoring gitignore and skipping
binary/oversized/empty files. Enumeration is sequential; the expensive
read+sha256 runs in parallel over rayon. A `previous` manifest lets an unchanged
file (matching mtime and size) reuse its prior hash with no disk read
(`src/manifest.rs:96`), so a re-run is a cheap negative check.

- [`Manifest::hashes`] (`src/manifest.rs:123`) is the set search intersects
  against.
- [`Manifest::signature`] (`src/manifest.rs:136`) is a sha256 over the sorted,
  deduped content-hash set: path-independent, so a pure rename does not change
  it (a content-addressed store needs no re-upload for a move), but an edit
  does. Sync compares it against the last synced signature to skip the network
  entirely when nothing changed.
- [`Db`] (`src/db.rs:37`) stores every checkout's entries in one shared SQLite
  DB at `<cache>/semantic-search/index.db` (`src/db.rs:224`), keyed by
  `(root, rel_path)`, in WAL mode with a busy timeout so several `search`
  processes share it. `save` is transactional and removes stale rows for that
  root only, never touching a sibling worktree's rows (`src/db.rs:143`). The
  `synced` table keys the signature by `(root, store)` so switching stores
  forces a fresh sync (`src/db.rs:207`).

## Dedup-aware sync (`src/sync.rs`)

[`sync`] (`src/sync.rs:64`) uploads every manifest file whose content is not
already in the store:

1. `ensure_store`, then list existing `external_id`s scoped by
   `source == code AND repo == <slug>` (`src/sync.rs:89`): listing the whole
   shared store unfiltered is the dominant first-run stall, so dedup is scoped
   per repo. A too-narrow scope only ever re-uploads a byte-identical blob (a
   cheap idempotent overwrite by content hash), never duplicates or corrupts.
2. Keep only hashes the store lacks, collapsing duplicate content within the
   checkout, refuse if over `max_files`, then upload at concurrency 16.
3. Each file becomes a code [`Document`] whose `external_id`, `content_hash`,
   and manifest hash are all the same value, with `source: "code"`, `repo`, and
   `path` in `meta_json` (`code_document`, `src/sync.rs:202`).
4. [`SyncReport`] (`src/sync.rs:39`) names `uploaded_ids` so the caller's wait
   covers only this run's files.

The deliberate departures from the prior `mgrep` tool, stated at
`src/sync.rs:1`: content addressing (one embedding per blob across N worktrees)
and never deleting in ordinary sync (deletion needs cross-manifest refcounting,
a separate GC pass).

[`wait_until_indexed`] (`src/sync.rs:245`) polls per-file status for exactly the
uploaded ids and returns once each has settled (embedded, failed, cancelled, or
deleted) or `timeout` elapses. It never consults the store's aggregate pending
counts, so an unrelated source's backlog cannot stall the wait (ENG-2699).

## The Store backend (`src/backend.rs`)

[`Store`] (`src/backend.rs:206`) abstracts the vector store so sync and search
run against `MemoryStore` in tests and `MixedbreadStore` in production. Methods
return `impl Future + Send` rather than `async fn` so callers can add the `Send`
bound the concurrent paths need (`src/backend.rs:8`). The surface:
`ensure_store`, `list_external_ids`, `list_records`, `upload`, `delete`,
`search`, `grep`, `list_chunks`, `facets`, `ask`, `store_status`, `file_status`.

Hit and option types live here: [`SearchHit`] with its [`Provenance`]
(`src/backend.rs:114`), [`SearchOptions`]/[`AskOptions`]/[`GrepOptions`]
(`src/backend.rs:25`), [`StoreStatus`], and [`StoredRecord`] (the listed record
with `content_hash`, `source`, `file_id`, `created_at` used by reconcile/GC,
`src/backend.rs:181`).

[`MemoryStore`] (`src/backend.rs:360`) is keyed by `external_id` like the real
store (so dedup-as-overwrite is exercised faithfully), evaluates metadata
filters, and tracks `upload_count` so tests can assert redundant syncs stay
flat.

[`MixedbreadStore`] (`src/adapter.rs:18`) wraps a `mixedbread::Client`. It maps
a `Document` to the client's upload and each `mixedbread::Chunk` back to a
`SearchHit`, reading `source`/`content_hash`/provenance from chunk metadata and
normalizing the API's 1-based `start_line` / line-span to a 0-based start and a
line count (`src/adapter.rs:122`). Build it with `from_env`
(`MXBAI_API_KEY`-only) or `from_login` (key, else `mgrep login` token,
`src/adapter.rs:33-49`). Inherent extras with no offline analogue:
`events_histogram` and `enhance_query` (`src/adapter.rs:59`).

## Pipeline and config

[`pipeline`](../indexer/overview.md) functions `index_and_semantic`,
`index_and_grep`, `index_and_answer` (`src/pipeline.rs:141`) are the
index-then-query flow: `prepare` builds and persists the manifest, skips sync
when the signature matches the last synced one for this `(base_url, store)`
(`src/pipeline.rs:79`), otherwise uploads and waits, and marks success on upload
acceptance (not embedding completion). A query whose sources exclude `code`
skips the worktree walk entirely (`src/pipeline.rs:72`). [`Config`]
(`src/config.rs`) carries the limits (`max_file_bytes` 1 MiB, `max_files`
10000); `DEFAULT_STORE = "index"` and `WEB_STORE = "mixedbread/web"`
(`src/config.rs:8`).

## Build

`package.nix` marks it `inRustWorkspace = true` with `passthruTests = true`: it
builds through the shared cargo-unit graph and its tests surface as a CI check.
No `default.nix`, no flake output; consumers take it as the
`search-core`/`search_core` workspace library.

See [internals](internals.md) for the query, projection, context, and
filter-building layer.

[`Manifest`]: #manifest-and-signature
[`FileEntry`]: #manifest-and-signature
[`ContentHash`]: #content-addressing-and-dedup
[`ContentHash::of_bytes`]: #content-addressing-and-dedup
[`Manifest::build`]: #manifest-and-signature
[`Manifest::hashes`]: #manifest-and-signature
[`Manifest::signature`]: #manifest-and-signature
[`Db`]: #manifest-and-signature
[`sync`]: #dedup-aware-sync-srcsyncrs
[`SyncReport`]: #dedup-aware-sync-srcsyncrs
[`wait_until_indexed`]: #dedup-aware-sync-srcsyncrs
[`Document`]: ../source/overview.md
[`Store`]: #the-store-backend-srcbackendrs
[`SearchHit`]: #the-store-backend-srcbackendrs
[`Provenance`]: #the-store-backend-srcbackendrs
[`SearchOptions`]: #the-store-backend-srcbackendrs
[`AskOptions`]: #the-store-backend-srcbackendrs
[`GrepOptions`]: #the-store-backend-srcbackendrs
[`StoreStatus`]: #the-store-backend-srcbackendrs
[`StoredRecord`]: #the-store-backend-srcbackendrs
[`MemoryStore`]: #the-store-backend-srcbackendrs
[`MixedbreadStore`]: #the-store-backend-srcbackendrs
[`Config`]: #pipeline-and-config
