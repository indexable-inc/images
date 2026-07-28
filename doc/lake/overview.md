# lake

`packages/lake` (member `lake/iceberg`, crate `lake-iceberg`) is the Iceberg
corpus lake: the durable, replayable log under the multi-source search corpus
(issue #752), succeeding the full-file-overwrite [parquet log](../sink/overview.md).
One table, `corpus.documents` (`NAMESPACE`/`TABLE`, `src/lib.rs:90`), holds an
append-only revision log of [`Document`](../source/overview.md) observations.
Library crate, no flake output; the [`indexer`](../indexer/overview.md) is its
sole caller. Both the write half (a [`Reconciler`](../source/overview.md)) and
the read half live in this one crate because they share the table schema, codec,
catalog connection, and fold (`src/lib.rs:19-22`).

## The log model (`src/lib.rs`, `src/codec.rs`)

Each reconcile pass appends only the documents that are new or changed, tagged
`op = upsert` (`OP_UPSERT`, `codec.rs:45`). Deletion is never inferred from
absence: a record missing from a newer pass stays live, so pruned local
transcripts or a reimaged host cannot erase already-folded history (ENG-2696,
`src/lib.rs:6-9`). The only deletes are explicit tombstones (`op = delete`,
`OP_DELETE`) that [`gc`](#write-half) appends for an export-complete source whose
complete input proves a record vanished.

One row is one observation by one writer **slice** (`host`, optional `user`,
`codec::Slice`). Current state is a per-slice fold ordered by `version`, the
slice's revision counter (its previous maximum plus one), which is committed data
so it survives compaction and wall-clock steps (`codec.rs:11-15`). An
`external_id` is live while any slice's latest op for it is an upsert, so one
host's tombstone cannot erase a record another slice still observes
(`src/lib.rs:11-16`). `observed_at` (epoch ms) is kept for queryability and as
the freshness arbiter between live replicas of one id in different slices, never
to order one slice's operations.

The first nine columns are exactly `sink-parquet`'s flat corpus schema, so an
existing polars/duckdb query ports by adding `op != 'delete'` to its filter;
`user`, `op`, `observed_at`, `version` are the log's additions
(`codec.rs:17-19`). A [`Document`] is reconstructed from `external_id`,
`content_hash`, `body`, `meta_json` alone (`CODEC_COLUMNS`, `codec.rs:68`).
Nullability rule: `content_hash`/`body`/`meta_json` are null exactly on a
tombstone; a null on an upsert row is a malformed log and a typed decode error
(`codec.rs:24-27`).

## Write half

[`IcebergReconciler`] (`src/lib.rs:234`): `new(catalog, ident, host)`
(`src/lib.rs:248`) sets the slice's host; `with_user(name)` (`src/lib.rs:259`)
derives a per-account reconciler. `reconcile` (`src/lib.rs:372`, the
`Reconciler` impl) diffs the writer's slice against the desired set and appends
only the new or changed documents (per id the fold keeps the newest
`content_hash`; ids absent from the pass simply remain). `gc`
(`src/lib.rs:317`) is the explicit deletion path: append tombstones for an
export-complete source's vanished ids. A conflicted append commit is retried up
to `COMMIT_ATTEMPTS` (5) times, each reloading the table and re-applying the
written data files (`src/lib.rs:94`). Returns [`Report`] (`src/lib.rs:205`).

## Read half

- [`read_state`] (`src/lib.rs:449`) folds the whole log into the current
  per-source [`Document`] sets ([`LakeState`], `src/lib.rs:430`), including
  sources whose records are all tombstoned, so a full rebuild can also
  garbage-collect a view.
- [`added_since`] (`src/lib.rs:518`) walks only the `Append` snapshots a cursor
  has not seen and returns a [`Delta`] (`src/lib.rs:484`) of upserts and
  tombstoned ids, so a steady-state view catch-up applies just the change
  (`Replace`/compaction snapshots rewrite files without adding rows, so the
  cursor walk follows `Append` only). An expired cursor (snapshot pruned from
  catalog metadata) is a typed error so the caller can fall back to `read_state`.
- [`current_snapshot_id`] (`src/lib.rs:473`) returns the cursor to persist for
  next time.

These feed the indexer's consume modes: `--from-iceberg` (full `read_state` ->
`replace`), `--from-snapshot`/`--cursor-file` (incremental `added_since` ->
`apply`).

## Build and connection

[`Config::connect`] (`src/lib.rs:121`) connects a REST catalog (production:
Cloudflare R2 Data Catalog) over an S3-compatible data plane, with S3
credentials from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`; tests run the
same code against iceberg's in-memory catalog. `ensure_table`
(`src/lib.rs:165`) creates the table if absent. It pins `iceberg` 0.9 (which
pulls arrow/parquet ^57, an isolated `parquet_57` tree distinct from the
workspace's 59, `Cargo.toml` / root `Cargo.toml:181`). `package.nix` is
`inRustWorkspace = true` with `passthruTests`; no flake output.

[`IcebergReconciler`]: #write-half
[`Report`]: #write-half
[`read_state`]: #read-half
[`LakeState`]: #read-half
[`added_since`]: #read-half
[`Delta`]: #read-half
[`current_snapshot_id`]: #read-half
[`Document`]: ../source/overview.md
[`Config::connect`]: #build-and-connection
