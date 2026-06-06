# iceberg-r2-spike

Phase 0.4 spike for [#752](https://github.com/indexable-inc/index/issues/752)
(corpus log → Apache Iceberg on Cloudflare R2). Answers, with running code, the
questions that gate the `sink-iceberg` / `source-iceberg` design. Standalone
workspace on purpose (own lockfile, like `packages/nix-cargo-unit`) so the
throwaway never touches the root lockfile, unit graph, or lint wall.

## Status

| Scenario | memory catalog | R2 Data Catalog |
|---|---|---|
| S1 create namespace + table | **PASS** | pending creds |
| S2/S3 append commits | **PASS** | pending creds |
| S4 full scan | **PASS** | pending creds |
| S5 snapshot-cursor incremental read | **PASS** | pending creds |
| S6 concurrent writers | **PASS** (serialized by the in-proc lock; real optimistic-concurrency conflicts need REST) | pending creds |
| compaction vs cursor, snapshot expiration | n/a locally | pending creds |

## Run

```sh
# local (memory catalog + tempdir warehouse)
cargo run

# against R2 Data Catalog
SPIKE_CATALOG=rest \
SPIKE_CATALOG_URI=https://catalog.cloudflarestorage.com/<account_id>/<bucket> \
SPIKE_WAREHOUSE=<account_id>_<bucket> \
SPIKE_CATALOG_TOKEN=<cloudflare api token> \
SPIKE_S3_ENDPOINT=https://<account_id>.r2.cloudflarestorage.com \
SPIKE_S3_ACCESS_KEY_ID=... SPIKE_S3_SECRET_ACCESS_KEY=... \
cargo run
```

The `SPIKE_S3_*` vars may be unnecessary if the catalog's `/config` response
vends storage config — testing that is part of the R2 half.

## Findings so far (each maps to a sink/source-iceberg design rule)

1. **File names must be unique per run.** `DefaultFileNameGenerator` with no
   suffix emits `prefix-00000.parquet` deterministically; the second commit is
   rejected with *"Cannot add files that are already referenced by table"*.
   `sink-iceberg` passes a per-run UUID suffix.
2. **The incremental cursor is a manifest walk we own (~30 lines), not a
   built-in scan mode.** iceberg-rust 0.9 has no `from_snapshot_id` incremental
   scan. `added_files_since(cursor)`: walk snapshots with
   `sequence_number > cursor's`, keep only `summary().operation == Append`,
   and within each snapshot keep only manifests with
   `added_snapshot_id == that snapshot's id`.
3. **Manifest files are immutable and carried forward** — without the
   `added_snapshot_id` filter the walk re-delivers every old file each run
   (observed: S5 returned both appends' files before the fix).
4. **`Replace` snapshots are compaction** (R2's managed compaction emits
   these); following them would re-deliver the whole table after every
   compaction. The cursor follows `Append` only.
5. **REST catalog in 0.9 requires an explicit `StorageFactory`**
   (`iceberg-storage-opendal`, `OpenDalStorageFactory::S3`) for the data
   plane; bearer auth is the `"token"` catalog prop; standard
   `iceberg::io::S3_*` props configure the S3 side.
6. **arrow version**: iceberg 0.9.1 pins arrow/parquet `^57.1`; the root
   workspace is on 58. Coexistence is fine for an isolated `sink-iceberg`
   crate (`multiple_crate_versions` is already allowed).

## R2 half still to verify

- Real commit-conflict behavior under concurrent appenders (S6 with optimistic
  concurrency instead of an in-proc lock) and the retry budget needed.
- Managed compaction emitting `Replace` snapshots the cursor correctly skips,
  and snapshot expiration invalidating a stale cursor (the
  "cursor not found → full rescan" path).
- Whether R2 Data Catalog vends S3 storage config via `/config` (no static S3
  keys needed) or explicit `SPIKE_S3_*` props are required.
- Catalog URI/warehouse string formats above are from R2 docs; confirm.
