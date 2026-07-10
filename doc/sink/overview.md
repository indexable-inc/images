# sink

`packages/sink` is the workspace of search-Document sinks: the write half of the
corpus, paired with the [source adapters](../source/overview.md). Each sink
implements [`source_meta::Reconciler`](../source/overview.md), converging one
view to a source's desired-state documents. Two members, both library crates (no
flake outputs, CI checks via `passthruTests`):

- `sink/mixedbread` (crate `sink-mixedbread`): reconcile a record source into a
  Mixedbread store (upload changed, skip unchanged, replace, or GC).
- `sink/parquet` (crate `sink-parquet`): write each source as one parquet file
  in an S3/R2 corpus log.

The [`lake-iceberg`](../lake/overview.md) reconciler is the third sink the
[`indexer`](../indexer/overview.md) fans out to; it lives in its own package
because its read and write halves share a schema and codec.

## sink-mixedbread (`sink/mixedbread/src/lib.rs`)

Records are addressed by a source-defined `external_id`; change detection
compares each document's `content_hash` against the value stored under that id
(`src/lib.rs:1-10`). [`MixedbreadReconciler`] (`src/lib.rs:115`) holds a
[`Store`](../search-core/overview.md) (production `MixedbreadStore`, tests
`MemoryStore`), the store name, and the embedding-wait timeout. Its shared upload
half `sync_source` (`src/lib.rs:152`):

1. `ensure_store`, then list the source's records scoped by `source == X`.
2. [`verify_scope`] (`src/lib.rs:92`): every returned record's own `source` must
   match; a backend that drops the filter aborts with `ScopeLeak` instead of
   feeding a store-wide delete set into replace/GC (it has happened: the API once
   renamed the list-filter parameter and ignored the old name).
3. Upload only the new or changed documents (a record with no stored hash
   predates hash tracking and re-embeds) at concurrency 16, then wait until those
   ids embed, gated on this pass's ids only (ENG-2699, `src/lib.rs:189`).

Four entry points over that half:

- `reconcile` (`src/lib.rs:474`, the `Reconciler` impl): upload changed, skip
  unchanged, KEEP remote absences. Used for live scans whose read can be
  transiently partial, where absence is not authoritative. Returns [`SyncReport`].
- `replace` (`src/lib.rs:241`): the log-replay sibling. Make the store's records
  for a source exactly `documents`, DELETING remote ids absent from the desired
  set, because a log fold's absences are explicit tombstones (including an empty
  set for a fully tombstoned source). Returns [`ReplaceReport`].
- `apply` (`src/lib.rs:279`): apply a log-derived delta (upserts + tombstones),
  trusting the log's change detection (no remote listing for skip decisions);
  idempotent, with deletes filtered against the store's current ids so a replayed
  cursor cannot wedge on a missing-id hard error. Returns [`ApplyReport`].
- `gc` (`src/lib.rs:354`): after a fully successful sync of an export-complete
  source, delete file objects whose `external_id` vanished from the COMPLETE
  produced set, plus exact-duplicate `(external_id, content_hash)` file objects
  from retried uploads, keeping the newest of each. `plan_gc` (`src/lib.rs:405`)
  derives the deletions purely from the scoped listing and the produced set (so
  the policy is testable without a store): condemn every file object of a vanished
  id, and trim a surviving id to one file object per content hash. Returns
  [`GcReport`].

## sink-parquet (`sink/parquet/src/lib.rs`)

A generic S3/R2 parquet sink: every source's documents share one flat schema
(one row per document), so the whole corpus is one polars/duckdb query regardless
of source; per-source extras live in the `meta_json` column rather than typed
columns (`src/lib.rs:1-6`). [`ParquetReconciler`] (`src/lib.rs:120`) writes one
file per source at `<prefix>/source=<source>/data.parquet`, rewritten in full
each run, with a sibling `_manifest.json` recording a content hash over the
source's `(external_id, content_hash)` set; a run whose corpus is unchanged skips
the rewrite (`src/lib.rs:8-12`). This trades incremental writes for idempotence
and zero dedup-on-read: a source's file always reflects its current desired
state. A very large source rewrites its whole file each change (sharding is a
future refinement). The consumer half that reads this log back is
[`source-parquet`](../source/adapters.md). [`Config::connect`] (`src/lib.rs:127`)
builds the `AmazonS3` store from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`;
the indexer host-partitions the prefix so many hosts sharing a bucket do not
clobber each other.

[`MixedbreadReconciler`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`verify_scope`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`SyncReport`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`ReplaceReport`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`ApplyReport`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`GcReport`]: #sink-mixedbread-sinkmixedbreadsrclibrs
[`ParquetReconciler`]: #sink-parquet-sinkparquetsrclibrs
[`Config::connect`]: #sink-parquet-sinkparquetsrclibrs
