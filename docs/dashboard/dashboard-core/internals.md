# dashboard-core internals: the Hub fold and recordings

The non-obvious mechanism behind the read-only canvas: how the [`Hub`] folds
many producers into one Loro document with cheap incremental diffs and free
replay, and how a [`RecordingStore`] persists that document. The public surface
(wire types, transport, server routes) is in [overview](overview.md).

## The shared document (`src/dashboard/hub.rs`)

[`Hub`] (`hub.rs:465`) owns one `LoroDoc` whose root `panes` map holds one entry
per pane, keyed `"<scope>\x1f<id>"` (`SCOPE_SEP = U+001F`, `hub.rs:45`; neither a
scope nor a pane id contains it, so the split back to `(scope, id)` is
unambiguous). A scope is one frame source: `tui::serve` uses a single `"local"`
scope, the aggregator one scope per producer.

Each pane entry is a `meta` `LoroMap` of scalars (`kind`, `created_at`, `title`,
`subtitle`, plus the view's own scalar fields) and one `LoroText` per large
mutable field the view declares. Storing each big field as its own text
container is what makes updates diff incrementally and the oplog replay: a Loro
oplog *is* a recording.

### Two projections, one reconcile loop

A view tells the hub its storage shape through two pure functions, so adding a
resource kind never touches the reconcile loop:

- [`view_scalars`] (`hub.rs:90`) returns the scalar `meta` fields per kind, using
  a `Scalar::Absent` sentinel to express "ensure this key is not present" (a
  terminal's `exit_code` before exit, an exec's `ok` while running) uniformly
  with the present cases.
- [`view_texts`] (`hub.rs:136`) returns the large text fields per kind: a
  terminal's `body` (its screen), an html `body`, an exec's
  `source`/`stdout`/`stderr`/`result`/`trace`, a data `body`. A data view's JSON
  and the exec `trace` are canonicalized to JSON text so they diff and replay
  like any body; the frontend parses them back.

The key set a view returns is fixed for its kind, so a slot created for one kind
always sees the same set on every later tick.

### Reconcile: `apply_scope` / `remove_scope`

[`Hub::apply_scope(scope, panes)`] (`hub.rs:483`, core at `DocState::apply_scope`
`hub.rs:265`) reconciles exactly the entries under `scope` to `panes` and leaves
every other scope alone:

1. For each pane, if a cached `Slot` exists under a *different* `kind`, drop it
   first so a reused id recreates cleanly rather than editing the wrong fields
   in place (`hub.rs:273`; test `kind_change_recreates_pane`, `hub.rs:761`).
2. `create_slot` (`hub.rs:300`) for a new key: insert the `meta` map, write
   `kind`, stamp `created_at` once with `now_ms()`, and create one empty text
   container per declared field.
3. `update_slot` (`hub.rs:341`) writes only changed values: `title`/`subtitle`,
   each scalar whose cached value differs, each text whose cached value differs.
   Caching is load-bearing: an unchanged insert is still a CRDT op, so the cache
   is what keeps an idle pane from producing a delta (test
   `unchanged_apply_yields_no_delta`, `hub.rs:583`). Sentinel initial values
   (`hub.rs:428`) force the first write even when the real value is empty.
4. Drop keys under this scope no longer present (`hub.rs:283`).
5. `commit_delta` (`hub.rs:403`).

[`Hub::remove_scope`] (`hub.rs:489`) drops every key under a prefix and commits;
removing an empty scope is a silent no-op (`hub.rs:643`). The multi-producer
invariant is `scopes_do_not_clobber_each_other` (`hub.rs:560`): one producer's
reconcile never touches another's panes, and disconnecting a producer removes
only its own.

A failed apply is dropped, not retried: the next tick re-renders
(`hub.rs:483-485`).

### Body diff thresholds

`set_text` (`hub.rs:178`) reconciles a text field. A body up to
`MAX_DIFF_BODY = 32 KiB` (`hub.rs:165`) uses Loro's refined diff under a
`BODY_DIFF_TIMEOUT_MS = 50.0` ceiling (`hub.rs:170`) for small deltas on the
common case (a terminal screen, appended exec output). A larger body, or any
diff that times out, is replaced wholesale (delete-all + insert), two bulk ops
whose cost is independent of string similarity, so a pane that rewrites
completely each tick (an html pane streaming a base64 data URL) cannot stall the
single aggregator or bloat the oplog. After a timeout the wholesale path
reconciles against the container's own current length, since the partial diff
may have applied edits before bailing (`hub.rs:187`).

### Timestamps and deltas

`DocState::new` (`hub.rs:245`) sets `set_record_timestamp(true)`.
`commit_delta` (`hub.rs:403`) stamps each commit with `set_next_commit_timestamp(now_ms())`,
commits, and exports `ExportMode::updates(&self.streamed)` only when the oplog
version moved, advancing `streamed`. So a tick that changes nothing yields
`None` and no broadcast. `now_ms()` (`hub.rs:54`) saturates rather than panics
on a clock before the epoch. Together the per-commit timestamp and the once-only
per-pane `created_at` (test `created_at_is_stamped_once`, `hub.rs:630`) give the
browser a fine-grained timeline axis and each resource's age with no producer
opt-in.

### Fan-out and snapshots

`Hub::broadcast` (`hub.rs:494`) base64-encodes a delta onto a
`broadcast` channel of capacity `BROADCAST_CAPACITY = 256` (`hub.rs:40`).
`Hub::subscribe` (`hub.rs:504`) returns the current full snapshot and the live
update receiver under one lock, so the snapshot version lines up with the first
update a subscriber sees (consumed by `server::events`). `snapshot_b64`
(`hub.rs:514`) re-snapshots a client the broadcast outran. [`Hub::export_snapshot`]
(`hub.rs:521`) returns the full snapshot bytes including the complete oplog, used
by the recorder; a snapshot is never shallow, so a receiver can replay any past
version, not just the latest (test `hub_snapshot_decodes_exec_pane_with_history`,
`tests/pipeline.rs:75`).

## Recordings (`src/dashboard/recordings.rs`)

The hub document is the recording: its oplog holds every change with a
millisecond timestamp, so one full snapshot replays the whole session. A
[`RecordingStore`] persists that snapshot to disk on an interval.

- **One file per run.** Each run owns `rec-<start-ms>.loro` (`PREFIX`/`EXT`,
  `recordings.rs:31`), rewritten in place as the document grows, not a growing
  count of files. [`RecordingStore::spawn_recorder`] (`recordings.rs:157`) prunes
  to `KEEP_RECORDINGS - 1` (50, `recordings.rs:27`), writes one snapshot up front
  so a session shorter than the interval still produces its advertised recording,
  then refreshes it each `interval`, returning a [`Recorder`] (`id` + task
  handle). A final snapshot on graceful shutdown is the caller's job (the
  aggregator does this; see [dashboard](../dashboard/overview.md)).
- **Listing.** [`RecordingStore::list`] (`recordings.rs:104`) returns
  [`RecordingInfo`] `{ id, started_ms, updated_ms, bytes }` newest first, parsing
  `started_ms` out of the id and `updated_ms` from the file mtime.
- **Default location.** [`RecordingStore::open_default`] resolves
  `$IX_DASH_RECORDINGS`, else `$XDG_STATE_HOME/ix-dash/recordings`, else
  `~/.local/state/ix-dash/recordings`, else `/tmp/ix-dash-recordings-<user>`
  (`recordings.rs:245`).

### Durability and security

- **Atomic writes.** [`RecordingStore::save`] (`recordings.rs:131`) writes a temp
  file then renames, so a reader never sees a half-written recording.
- **Owner-only.** The directory is set `0700` and each file is created `0600`
  via `write_private` (`recordings.rs:227`); recordings capture exec source,
  stdout, and stderr, so they must not be world-readable even briefly, and the
  tight directory blocks a symlink-plant before the rename.
- **Id validation.** `path_for` (`recordings.rs:188`) accepts only a bare
  `rec-<digits>` stem (no separators, no `..`), so `load`/`save` and the
  `GET /recording/{id}` route can never read or write outside the store (test
  `rejects_unsafe_ids`, `recordings.rs:310`).

A store failure is non-fatal to the aggregator: the dashboard still serves live
and replay works from the browser's own captured history.

[overview]: overview.md
[`Hub`]: overview.md
[`Hub::apply_scope`]: overview.md
[`Hub::apply_scope(scope, panes)`]: overview.md
[`Hub::remove_scope`]: overview.md
[`Hub::export_snapshot`]: overview.md
[`view_scalars`]: #two-projections-one-reconcile-loop
[`view_texts`]: #two-projections-one-reconcile-loop
[`RecordingStore`]: overview.md
[`RecordingStore::list`]: #recordings-srcdashboardrecordingsrs
[`RecordingStore::save`]: #durability-and-security
[`RecordingStore::open_default`]: #recordings-srcdashboardrecordingsrs
[`RecordingStore::spawn_recorder`]: #recordings-srcdashboardrecordingsrs
[`Recorder`]: overview.md
[`RecordingInfo`]: overview.md
