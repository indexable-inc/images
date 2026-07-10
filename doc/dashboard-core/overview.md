# dashboard-core

`packages/dashboard-core` is the engine-free crate every dashboard process
links. It holds the wire types and discovery paths a producer and a consumer
agree on, the unix-socket [`Publisher`]/[`subscribe`] transport, the
browser-facing [`Hub`] Loro document and its SSE server, the durable
[`RecordingStore`], and the embedded single-page canvas. A process that only
publishes panes pulls in no HTTP or CRDT code; only a consumer that serves the
board touches the `dashboard` module.

It is a workspace library crate (`packages/dashboard/dashboard-core/package.nix`:
`inRustWorkspace = true`, no `flake`/`packageSet`), so it has no flake output of
its own. It is consumed by [`dashboard`](../dashboard/overview.md),
[`ix-windows`](../ix-windows/overview.md), and the `tui` crate. The deep
reconcile mechanism and recordings are documented in [internals](internals.md).

## Modules (`src/lib.rs:28-43`)

- **`pane`** - the wire types ([`Pane`], [`View`] and its variants,
  [`ProducerSnapshot`]) and the discovery paths ([`discovery_dir`],
  [`socket_path`]). Both halves of the system depend on this and nothing else of
  each other. `src/pane.rs`.
- **`publish`** - producer side: [`Publisher`] and the cloneable [`PaneSink`]
  that stream a snapshot over a `UnixListener`. `src/publish.rs`.
- **`subscribe`** - consumer side: [`subscribe`] discovers sockets and streams
  [`ProducerEvent`]s. `src/subscribe.rs`.
- **`dashboard`** - the read-only web canvas: the [`Hub`] document
  (`src/dashboard/hub.rs`), the HTTP/SSE [`serve_hub`] server
  (`src/dashboard/server.rs`), and the [`RecordingStore`]
  (`src/dashboard/recordings.rs`). See [internals](internals.md).
- **`error`** - one [`Error::Dashboard`] variant collapsing foreign-boundary
  failures (TCP bind, Loro encode); `Result<T>` alias. `src/error.rs`.

## Wire types (`src/pane.rs`)

A [`Pane`] (`pane.rs:35`) is `{ id, title, subtitle, view }`. `id` is unique
within a producer; the consumer namespaces it. Constructors set sensible titles:
`Pane::terminal`/`html`/`exec`/`data` (`pane.rs:50-105`).

[`View`] (`pane.rs:115`) is an internally tagged enum, `#[serde(tag = "kind",
rename_all = "snake_case")]`, so each body serializes with a `kind`
discriminant and the browser renders by tag with no schema negotiation:

| `kind` | type | body |
| --- | --- | --- |
| `terminal` | [`TerminalView`] (`pane.rs:141`) | `command`, `args`, `rows`/`cols`, `alive`, `screen` (rows newline-joined with minimal ANSI SGR runs), cursor row/col/visible/shape, `exit_code`. |
| `html` | [`HtmlView`] (`pane.rs:185`) | one self-contained `html` document, mounted sandboxed. |
| `exec` | [`ExecView`] (`pane.rs:200`) | one captured process run: `source`, `lang`, `stdout`, `stderr`, `result`, `running`, `ok`, `duration_ms`, and an inline `trace` of [`ExecTraceLine`] (`pane.rs:242`) pairing each output chunk with the 1-based source line that emitted it. |
| `data` | [`DataView`] (`pane.rs:268`) | arbitrary JSON `data` plus a `renderer` name; an unknown/empty name falls back to a generic JSON tree on the frontend. |

[`View::kind`] (`pane.rs:129`) returns the wire tag. Adding a first-class
resource adds a `View` variant and a native renderer; a user-defined one reuses
`Html`/`Data` with no aggregator change. Fields added after the first wire shape
carry `#[serde(default)]` so a mixed-version fleet keeps parsing (`pane.rs:156`;
test `old_terminal_wire_shape_deserializes_with_field_defaults`, `pane.rs:346`).

[`ProducerSnapshot`] (`pane.rs:285`) is `{ producer, panes }`: one producer's
full pane set, the unit streamed over a socket.

### Discovery paths

- [`discovery_dir`] (`pane.rs:300`): `$IX_DASH_DIR`, else
  `$XDG_RUNTIME_DIR/ix-dash`, else `/tmp/ix-dash-<user>`. Short by design: macOS
  caps a unix `sun_path` at 104 bytes, and `$TMPDIR` would blow that budget.
- [`socket_path`] (`pane.rs:316`): `<discovery_dir>/<pid>-<short-uuid>.sock`.

## Producer side: `publish` (`src/publish.rs`)

[`Publisher::bind(path, runtime)`] (`publish.rs:109`) binds a `UnixListener` at
`path` (usually [`socket_path`]) and spawns an accept loop on the supplied
runtime handle, so a producer can bind from a thread that is not itself a tokio
worker (e.g. one driving a native run loop). It creates a missing parent dir
mode `0700`, reaps a stale socket left by a crashed producer, and refuses to
overwrite a non-socket at `path` (`reap_stale_socket`, `publish.rs:281`). The
socket is set mode `0600`.

[`Publisher::publish(&panes)`] (`publish.rs:176`) and the cloneable
[`PaneSink::publish`] (`publish.rs:65`) replace the streamed snapshot. Each is
cheap and synchronous: it serializes one NDJSON line into a `watch` channel; per
connection a `write_loop` (`publish.rs:254`) writes the current line then each
new one. A background sampling loop holds a [`PaneSink`] cloned from
[`Publisher::sink`] (`publish.rs:184`) and is attached with
[`Publisher::push_task`] so it stops with the publisher. [`Publisher::stop`]
(`publish.rs:210`) / `Drop` signal the loops, abort tasks, and unlink the
socket, so the aggregator stops listing a dead producer. [`Publisher::path`] and
[`Publisher::producer_id`] expose the bound socket and the producer id.

## Consumer side: `subscribe` (`src/subscribe.rs`)

[`subscribe(dir, rescan, handle)`] (`subscribe.rs:53`) returns an
`mpsc::Receiver<ProducerEvent>` (channel depth `256`, `subscribe.rs:43`). The
`discover` loop (`subscribe.rs:63`) rescans `dir` every `rescan`, and for each
new `*.sock` spawns exactly one reader (`connected` set, so a re-created socket
reconnects only after its reader finishes). `read_producer` (`subscribe.rs:100`)
connects, parses each NDJSON line, and forwards a
[`ProducerEvent::Snapshot`]; on hangup it emits one
[`ProducerEvent::Gone { producer }`] (`subscribe.rs:28`). A malformed line is
skipped, not fatal (`subscribe.rs:121`); a connection refused on an actual
socket reaps the stale file but never deletes a user's regular `*.sock`
(`is_socket`, `subscribe.rs:138`). Dropping the receiver winds the loops down.
Both the aggregator and `ix-windows` consume this one transport
(`subscribe.rs:9`).

## Web surface: `serve_hub` (`src/dashboard/server.rs`)

[`serve_hub(hub, addr, recordings, runtime)`] (`server.rs:138`) binds a
`TcpListener`, starts an `axum` server on `runtime`, and returns a
[`ServedDashboard`] = a [`Dashboard`] handle plus a `watch` shutdown receiver
the caller threads into its own frame-source tasks. One owner for the router
means `tui::serve` and the standalone aggregator render through the same page and
stream. Routes (`server.rs:155`):

| route | handler | response |
| --- | --- | --- |
| `GET /`, `/index.html` | `index` | the embedded `DASHBOARD_HTML` page (`server.rs:40`, `:184`). |
| `GET /events` | `events` | SSE: one `snapshot` event (base64 Loro snapshot) then `update` events (base64 deltas); a lagged client is re-sent a fresh `snapshot` (`server.rs:211`). |
| `GET /recordings` | `list_recordings` | JSON list of [`RecordingInfo`], newest first; empty without a store (`server.rs:190`). |
| `GET /recording/{id}` | `get_recording` | one recording's snapshot bytes (`application/octet-stream`); a bad or traversing id is a 404, validated by the store (`server.rs:201`). |

[`Dashboard`] (`server.rs:63`) exposes `addr()`/`url()` (the bound `:0` port is
resolved), `push_task` to tie a frame-source task's lifetime to the server, and
`stop()` which aborts (not just signals) tasks: an open SSE stream never ends on
its own, so `axum` graceful shutdown would block forever otherwise
(`server.rs:91`). `recordings = None` leaves the replay routes reporting an empty
list, which is what `tui::serve` passes.

The [`Hub`] document fold, the projections that let a new view kind skip the
reconcile loop, and the [`RecordingStore`] durability/security model are in
[internals](internals.md).

## Public surface

Re-exported from `src/lib.rs:34-43`:

- types: [`Pane`], [`View`], [`TerminalView`], [`HtmlView`], [`ExecView`],
  [`ExecTraceLine`], [`DataView`], [`ProducerSnapshot`]
- paths: [`discovery_dir`], [`socket_path`]
- producer: [`Publisher`], [`PaneSink`]
- consumer: [`subscribe`], [`ProducerEvent`]
- web: [`Hub`], [`serve_hub`], [`Dashboard`], [`ServedDashboard`]
- recordings: [`RecordingStore`], [`Recorder`], [`RecordingInfo`]
- errors: [`Error`], [`Result`]

## The embedded page (`src/dashboard/server.rs:40`, `build.rs`)

`DASHBOARD_HTML` is `include_str!(concat!(env!("OUT_DIR"),
"/dashboard.html"))`. The UI source is a Svelte/Vite app under `site/` whose
renderer registry (`site/src/lib/renderers.ts`) maps each pane `kind` to a body
component (`terminal`/`html`/`exec`/`data`) and dispatches a `data` pane's
`renderer` name (e.g. `namespace`) to a named component. The frontend keeps the
whole Loro oplog so it can scrub to any past version and replay
(`site/src/lib/stream.svelte.ts`). Nix builds the app to one self-contained file
(`ix.buildSvelteSite`, `lib/rust/workspace.nix:39`), exposes it through
`IX_DASHBOARD_SITE_HTML` (`workspace.nix:218`), and `build.rs` (`build.rs:32`)
copies it into `OUT_DIR`. Compile-time embedding is deliberate: `dashboard-core`
is linked into non-wrappable artifacts (the `tui-py` PyO3 `.so` the MCP loads)
that cannot carry a runtime `--site-dir`. A bare `cargo build` with no env var
embeds a stub page (`build.rs:27`).

## Tests

- `src/pane.rs:321` - wire round-trips, the old-shape default-fill, exec titling.
- `src/publish.rs:309`, `src/subscribe.rs:143` - stale-socket reaping refuses a
  regular file; a producer that publishes then drops yields `Snapshot` then
  `Gone`.
- `tests/pipeline.rs` - end-to-end: a real producer socket streams an exec pane,
  the hub fold + Loro snapshot decode preserves captured output and full history,
  a recording round-trips through disk, and the HTTP server serves the page and
  lists a recording.

[internals]: internals.md
[`Error::Dashboard`]: #
[`Error`]: #
[`Result`]: #
[`Pane`]: #wire-types-srcpanrs
[`View`]: #wire-types-srcpanrs
[`View::kind`]: #wire-types-srcpanrs
[`TerminalView`]: #wire-types-srcpanrs
[`HtmlView`]: #wire-types-srcpanrs
[`ExecView`]: #wire-types-srcpanrs
[`ExecTraceLine`]: #wire-types-srcpanrs
[`DataView`]: #wire-types-srcpanrs
[`ProducerSnapshot`]: #wire-types-srcpanrs
[`discovery_dir`]: #discovery-paths
[`socket_path`]: #discovery-paths
[`Publisher`]: #producer-side-publish-srcpublishrs
[`Publisher::bind`]: #producer-side-publish-srcpublishrs
[`Publisher::bind(path, runtime)`]: #producer-side-publish-srcpublishrs
[`Publisher::publish`]: #producer-side-publish-srcpublishrs
[`Publisher::publish(&panes)`]: #producer-side-publish-srcpublishrs
[`Publisher::sink`]: #producer-side-publish-srcpublishrs
[`Publisher::stop`]: #producer-side-publish-srcpublishrs
[`PaneSink`]: #producer-side-publish-srcpublishrs
[`PaneSink::publish`]: #producer-side-publish-srcpublishrs
[`subscribe`]: #consumer-side-subscribe-srcsubscribers
[`subscribe(dir, rescan, handle)`]: #consumer-side-subscribe-srcsubscribers
[`ProducerEvent`]: #consumer-side-subscribe-srcsubscribers
[`ProducerEvent::Snapshot`]: #consumer-side-subscribe-srcsubscribers
[`ProducerEvent::Gone`]: #consumer-side-subscribe-srcsubscribers
[`ProducerEvent::Gone { producer }`]: #consumer-side-subscribe-srcsubscribers
[`serve_hub`]: #web-surface-serve_hub-srcdashboardserverrs
[`serve_hub(hub, addr, recordings, runtime)`]: #web-surface-serve_hub-srcdashboardserverrs
[`Dashboard`]: #web-surface-serve_hub-srcdashboardserverrs
[`ServedDashboard`]: #web-surface-serve_hub-srcdashboardserverrs
[`Hub`]: internals.md
[`RecordingStore`]: internals.md
[`Recorder`]: internals.md
[`RecordingInfo`]: internals.md
