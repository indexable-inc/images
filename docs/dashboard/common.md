# Dashboard

A live, replayable web canvas for every resource an ix process exposes. Any
producing process (a `tui` PTY manager, the MCP, a demo binary) writes its
current panes to a unix socket in a shared discovery directory. Consumers watch
that directory, connect to every socket, and render the fleet: the standalone
[dashboard](dashboard/overview.md) aggregator folds every producer into one Loro
document and serves it over HTTP + Server-Sent Events, while
[ix-windows](ix-windows/overview.md) opens one borderless native webview window
per live MCP resource. Both consumers and every producer build on one
engine-free crate, [dashboard-core](dashboard-core/overview.md), so a process
that only publishes panes links no HTTP, CRDT, or native engine.

Read this page first, then the component pages it links.

## Units

| unit | kind | role |
| --- | --- | --- |
| `packages/dashboard-core` | Rust lib crate (workspace member, no flake output) | wire types ([`Pane`]/[`View`]/[`ProducerSnapshot`]), discovery paths, the [`Publisher`]/[`subscribe`] socket transport, the [`Hub`] Loro document + SSE server ([`serve_hub`]), the [`RecordingStore`], and the embedded Svelte canvas page. See [dashboard-core](dashboard-core/overview.md). |
| `packages/dashboard` | Rust binary (`nix run .#dashboard`) | standalone aggregator: scan the discovery dir, fold every producer into one `Hub`, serve the board, persist recordings. Carries a `demo` subcommand. See [dashboard](dashboard/overview.md). |
| `packages/ix-windows` | Rust binary + lib (`nix run .#ix-windows`, darwin-only) | tao+wry consumer: render each `resource/<id>` html pane as its own chrome-less native window. See [ix-windows](ix-windows/overview.md). |

`dashboard-core` is the only shared dependency; `dashboard` and `ix-windows`
each depend on it and on nothing else in this domain. The `tui` crate (other
domain) re-exports these names and adapts its PTY manager into terminal panes;
the MCP's `pane_bridge.py` (other domain) is the Python producer.

## How it fits together

```
producers (out of process)                consumers
  tui PTY manager  --\                 /-- dashboard  --> Hub (Loro doc) --> HTTP + SSE --> browser
  mcp pane_bridge  ---> *.sock files --+                                 \-> RecordingStore (rec-*.loro)
  dashboard demo   --/   (discovery dir)  \-- ix-windows --> WindowManager --> one webview window per resource
```

1. A producer builds a `Vec<Pane>` and calls [`Publisher::bind`] +
   [`Publisher::publish`] (`packages/dashboard-core/src/publish.rs:109`,
   `:176`). The publisher binds a `UnixListener` in the discovery directory and
   streams the latest [`ProducerSnapshot`] as one NDJSON line to every connected
   reader. Replacement semantics: the newest line fully describes that producer,
   so a late-joining consumer needs no backlog.
2. A consumer calls [`subscribe`] (`packages/dashboard-core/src/subscribe.rs:53`),
   which rescans the directory on an interval, connects to each `*.sock`, parses
   each line, and forwards a [`ProducerEvent::Snapshot`] or, on hangup, a
   [`ProducerEvent::Gone`]. One reader task per socket; a re-created socket
   reconnects on the next scan. Both consumers share this one transport.
3. The [dashboard](dashboard/overview.md) aggregator applies each event to a
   shared [`Hub`] under the producer's own scope ([`Hub::apply_scope`] /
   [`Hub::remove_scope`], `src/dashboard/hub.rs:483`, `:489`) and serves the
   document via [`serve_hub`]. [ix-windows](ix-windows/overview.md) instead feeds
   each event to a `WindowManager` that opens, refreshes, and closes native
   windows.

## Cross-component invariants

- **Discovery directory** is resolved identically everywhere by
  [`discovery_dir`] (`pane.rs:300`): `$IX_DASH_DIR`, else
  `$XDG_RUNTIME_DIR/ix-dash`, else `/tmp/ix-dash-<user>`. Kept short because
  macOS caps a `sun_path` at 104 bytes. Producers create it mode `0700`; sockets
  are mode `0600`.
- **Naming.** A producer id is `"<pid>-<short-uuid>"` (`publish.rs:35`); its
  socket file is `"<pid>-<short-uuid>.sock"` ([`socket_path`], `pane.rs:316`). A
  pane `id` is unique only within its producer; consumers namespace it by
  producer for a global key.
- **Wire forward-compatibility.** New view fields are `#[serde(default)]`
  (`pane.rs:162`), so a producer built before a field still parses; a consumer
  skips a malformed line rather than dropping the producer
  (`subscribe.rs:121`).
- **Engine-free producers.** `dashboard-core` carries no native engine; the page
  is embedded at compile time (below), and the producer half holds no HTTP or
  CRDT dependency, so a process that only publishes stays lightweight.
- **Scope isolation.** In the aggregator's `Hub`, one frame source is one scope;
  reconciling a scope never touches another's panes, and a disconnected producer
  removes only its own (`src/dashboard/hub.rs:265`, test at `:560`).
- **Read-only sync.** Browsers never write back to the Loro document; it has one
  editor per scope, so conflict resolution never runs. The CRDT is used only for
  cheap incremental diffs, single-snapshot catch-up, and free replay of the
  timestamped oplog.

## The page is embedded, not committed

The browser UI is a Svelte/Vite app under
`packages/dashboard-core/site/`, built by Nix (`ix.buildSvelteSite`) to one
self-contained `index.html` (viteSingleFile). The build points the workspace at
that file through `IX_DASHBOARD_SITE_HTML` (`lib/rust/workspace.nix:218`), and
`dashboard-core`'s `build.rs` copies it into `OUT_DIR` so `server.rs`
`include_str!`s it at compile time (`build.rs:32`, `server.rs:40`). A bare
`cargo build` outside Nix embeds a stub page. There is no committed artifact and
no runtime asset directory, which is why the in-process `tui::serve` and a
PyO3 `.so` can carry the page too.

## Glossary

- **producer**: a process that publishes its panes over a socket via
  [`Publisher`]. It owns its resources; it never owns the server.
- **consumer**: a process that reads producer sockets via [`subscribe`]. The
  aggregator and `ix-windows` are the two consumers.
- **pane**: one titled card on the canvas ([`Pane`], `pane.rs:35`): an `id`, a
  `title`, a `subtitle`, and a [`View`] body.
- **view**: a pane's body, a tagged union over render strategies: `Terminal`,
  `Html`, `Exec`, `Data` ([`View`], `pane.rs:115`). The aggregator stores it by
  `kind` and never learns what it means.
- **producer snapshot**: a producer's full current pane set on the wire
  ([`ProducerSnapshot`], `pane.rs:285`); one NDJSON line, replacement semantics.
- **scope**: the key a frame source's panes live under in the hub document; one
  per producer for the aggregator, the single `"local"` scope for `tui::serve`.
- **hub**: the shared Loro document plus its SSE fan-out ([`Hub`],
  `src/dashboard/hub.rs:465`).
- **recording**: a persisted Loro snapshot of a board run (`rec-<start-ms>.loro`),
  written on an interval by a [`RecordingStore`] and replayable in the browser.
- **aggregator**: the `dashboard` binary, the multi-producer consumer that serves
  the web board.

## Components

| component | page | what |
| --- | --- | --- |
| dashboard-core | [dashboard-core/overview.md](dashboard-core/overview.md) | wire types, discovery, publish/subscribe transport, Hub + SSE server, recordings, embedded page |
| dashboard | [dashboard/overview.md](dashboard/overview.md) | standalone aggregator binary: fold every socket into one served board; `demo` subcommand |
| ix-windows | [ix-windows/overview.md](ix-windows/overview.md) | darwin webview consumer: one borderless native window per live MCP resource |

[`Pane`]: dashboard-core/overview.md
[`View`]: dashboard-core/overview.md
[`ProducerSnapshot`]: dashboard-core/overview.md
[`Publisher`]: dashboard-core/overview.md
[`Publisher::bind`]: dashboard-core/overview.md
[`Publisher::publish`]: dashboard-core/overview.md
[`subscribe`]: dashboard-core/overview.md
[`ProducerEvent::Snapshot`]: dashboard-core/overview.md
[`ProducerEvent::Gone`]: dashboard-core/overview.md
[`Hub`]: dashboard-core/overview.md
[`Hub::apply_scope`]: dashboard-core/overview.md
[`Hub::remove_scope`]: dashboard-core/overview.md
[`serve_hub`]: dashboard-core/overview.md
[`RecordingStore`]: dashboard-core/overview.md
[`discovery_dir`]: dashboard-core/overview.md
[`socket_path`]: dashboard-core/overview.md
