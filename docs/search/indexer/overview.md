# indexer

`packages/indexer` syncs every configured corpus source into Mixedbread (the
semantic search index) and a durable corpus log (the S3/R2 parquet archive
and/or the [Iceberg lake](../lake/overview.md)). It is the only writer of the
shared corpus. Binary `indexer` (`nix run .#indexer`, `default.nix`), plus a
home-manager module `services.indexer` (`home-module.nix`).

Each source is an adapter implementing
[`source_meta::SourceAdapter`](../source/overview.md); the indexer drives them
and fans documents out to [sinks](../sink/overview.md) (`src/main.rs:1-20`). The
log-as-source-of-truth model: the parquet/Iceberg log is the append-only truth,
the Mixedbread index a materialized view replayable from it (issues #736, #752).

## Sinks and modes

`main` (`src/main.rs:232`) connects up to three sinks once per run (so a bad
config fails at startup, not mid-source): the Mixedbread store
(`--mixedbread-store`, `MixedbreadReconciler`), the S3/R2 parquet sink
(`--bucket`, `ParquetReconciler`), and the Iceberg lake (`--catalog-uri`,
`IcebergReconciler`). At least one sink and one source are required.

Two top-level paths:

- **Scan** (default): open each selected source, read its `documents()` once, and
  reconcile into every sink (`run_sources`, `src/main.rs:795`).
- **Consume** (exactly one of `--from-parquet-prefix`, `--from-iceberg`,
  `--from-snapshot`, `--cursor-file`, mutually exclusive, `src/main.rs:260`):
  replay a corpus log back into Mixedbread/the lake instead of scanning local
  sources (`run_consume_mode`, `src/main.rs:350`).

## Source selection

History sources (per host/user): `--local` (claude, codex, atuin at default
paths), `--claude-dir`, `--codex-file`, `--codex-sessions`, `--atuin-db`,
`--journald-since` (`src/main.rs:125-169`). Bulk exports:
`--slack-export`, `--linear-export`, `--github-export`, `--git-repo`
(repeatable). Code: `--code-repo` (repeatable, Mixedbread only, content-addressed
like a bare `search`, `src/main.rs:171`). Multi-user: `--user NAME:HOME`
(repeatable) lets one root process index every account, tagging each user's
records (`src/main.rs:176`); symlinked history paths are skipped so a privileged
run cannot be a confused deputy (`safe_path_under`, `src/main.rs:1450`).

`--host` tags records (default the system hostname). `--gc` runs a
garbage-collection pass after a successful sync for export-complete sources
(slack, linear, github, git): delete records whose `external_id` vanished from
this run's complete input, plus exact-duplicate file objects from retried
uploads; with the lake active, the vanished set is appended as explicit
tombstones (the lake's only deletion path, ENG-2696, `src/main.rs:188`). The
append-only history sources are never GC'd: their scans are incremental windows,
not complete snapshots.

## One pass, fan-out (`run_source`, `src/main.rs:1593`)

`run_source` reads the adapter's `documents()` exactly once into a `Vec`
(`src/main.rs:1613`) and reconciles that one set into the parquet, lake, and
Mixedbread sinks in turn. A selected source with no sink is an error, not a
silent no-op. An adapter parse error fails the source before any sink is touched;
a sink error is collected so a second sink still runs, and every sink failure is
surfaced. It returns the produced `external_id` set so `--gc` can diff against
it. Codex's two inputs (prompt log + session rollouts) are one adapter and one
`run_source` so they share a reconcile and cannot clobber each other
(`src/main.rs:841`).

## Scan cursor (`src/scan_cursor.rs`)

Per-`(user, source)` input-file cursor (ENG-2698): a history source whose input
files (size + mtime) are unchanged since its last successful run is skipped
without re-parsing a single transcript. `snapshot` (`scan_cursor.rs:58`) lstats
every regular file a source would read (no symlinks below a named root);
`ScanCursor::unchanged` (`scan_cursor.rs:161`) compares against the stored
snapshot; `store` (`scan_cursor.rs:191`) writes it atomically (temp + rename)
only after every sink succeeded. The skip is all-or-nothing per source: the
durable sinks consume the complete document set (parquet overwrites in full, the
lake derives tombstones from absences), so a partial parse would erase the
documents of every file it left out (`scan_cursor.rs:9`). Cursor state lives
under `--cursor-dir` (the fleet passes systemd's `$STATE_DIRECTORY`); an absent
or malformed cursor just forces a full reingest.

## Consume modes (`run_consume_mode`, `src/main.rs:350`)

- `--from-iceberg`: fold the whole lake into its current per-source sets and
  REPLACE each source's Mixedbread records (deleting view records the lake has
  tombstoned), via `MixedbreadReconciler::replace`.
- `--from-snapshot <id>`: apply the lake's changes since an explicit cursor
  (stateless, the caller owns the cursor); prints the snapshot to use next.
- `--cursor-file <path>`: steady-state lake catch-up; read the cursor, apply the
  delta, write the new cursor back; an absent/expired cursor falls back to a full
  rebuild (`run_cursor_consume`, `src/main.rs:499`).
- `--from-parquet-prefix`: read the per-source `data.parquet` files
  [`sink-parquet`](../sink/overview.md) wrote and reconcile them into Mixedbread,
  or, with `--catalog-uri`, fold the parquet archive into the lake
  (`fold_parquet_into_lake`, `src/main.rs:1135`).

Lake reads (`read_state`, `added_since`) and writes come from
[lake-iceberg](../lake/overview.md); the parquet reader is
[source-parquet](../source/overview.md).

## Config and auth

S3/R2 sink flags: `--bucket`, `--endpoint`, `--region` (default `auto`),
`--prefix` (default `corpus`); keys are host/user/source hive partitions so many
hosts indexing the same account (e.g. `root`) into one bucket do not clobber each
other (`src/main.rs:293-313`). Lake flags: `--catalog-uri`, `--warehouse`,
`--catalog-token`. Auth: Mixedbread via `MXBAI_API_KEY` else the `mgrep login`
token; S3 via `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`. `INDEX_TIMEOUT` is
2 minutes for embedding waits (`src/main.rs:44`).

## NixOS / home-manager module

`home-module.nix` exposes `services.indexer`: run the indexer on a timer (default
daily) as the current user via the portable-services module (a launchd agent on
macOS, a systemd user unit + timer on Linux). Options map one-to-one onto the
CLI flags (`local`, `claudeDir`, ..., `bucket`, `mixedbreadStore`, `interval`,
`environment`). It asserts at least one sink and one source, and warns that
`environment` is rendered into the world-readable Nix store, so secrets should
come from a runtime wrapper or the `mgrep login` token, never inlined
(`home-module.nix:12-18`).

## Build

`default.nix` selects the `indexer` binary from the workspace graph;
`package.nix` sets `flake`/`packageSet`, so `nix run .#indexer` and `pkgs.indexer`.
It depends on every `source-*` adapter, `search-core`, `sink-mixedbread`,
`sink-parquet`, `lake-iceberg`, and the `mixedbread` client (`Cargo.toml:15-31`).
