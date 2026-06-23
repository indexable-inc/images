# nix-web-monitor

`packages/nix-web-monitor` runs a Nix command with quiet terminal output and a
live browser monitor: a build tree, log tail, activity DAG, store-optimisation
totals, and a `nix-daemon` syscall panel, all on one HTTP port. It is two Rust
workspace crates:

- **`parser`** (`nix-web-monitor-parser`): the pure, testable parser and state
  model for Nix's `internal-json` event stream. No I/O. Also exports the
  per-tracer syscall line parsers in its `daemon` module.
- **`server`** (`nix-web-monitor`): the `axum` binary that spawns `nix`, feeds
  stderr through the parser, resolves dependency edges, samples daemon syscall
  hot paths, and streams deltas to browsers over a WebSocket. Wrapped with a
  built Svelte site.

The state-machine, transport, dependency-resolution, and daemon-tracing mechanics
are in [internals](internals.md).

## Why it exists

Nix's `--log-format internal-json` is a flat event stream: it names each
derivation built but carries no dependency edges, no per-activity success marker,
and goes silent inside a single long `addToStore` (writing a path, or
hard-linking every file under `auto-optimise-store`), which is exactly when a
build looks hung. nix-web-monitor reconstructs the build DAG out-of-band, folds
content-addressed `resolved derivation` pairs into one row, measures the
otherwise-unsized "copying to the store" source, and taps the daemon's syscalls
so the silent phase is visible. The browser feed is plain `ws://` on the page's
own origin, so off-host access (LAN/Tailscale) needs no certificate.

## CLI surface (`server/src/main.rs:42`)

```
nix-web-monitor [--host H] [--port N] [--exit-when-done]
                [--terminal-output summary|logs|quiet] [--nix-verbose]
                -- <nix args...>
```

- `--host` (default `0.0.0.0`, `:49`) / `--port` (default `7532`, `:53`): the UI,
  the `/api/state` JSON snapshot, and the `/ws` delta feed all share this port.
- `--exit-when-done` (`:61`): exit when the Nix command finishes instead of
  keeping the UI alive for inspection.
- `--terminal-output` (`:65`): `summary` (URL, plain output, warnings/errors,
  status), `logs` (also parsed build logs), or `quiet` (wrapper status only). The
  browser always gets the full parsed stream regardless.
- `--nix-verbose` (`:69`): pass `-v` to Nix for richer activity events.
- trailing args (`:73`): forwarded to `nix` verbatim, e.g. `build .#ix
  --keep-going`. The wrapper runs whatever `nix` is on the operator's PATH so the
  build matches a bare `nix build`.
- `--version` carries the shared build stamp via
  [build-version](../build-version/overview.md) (`server/src/main.rs:107`).

HTTP routes (`server/src/main.rs:189`): `/` serves `index.html` with
`Cache-Control: no-store` (so a rebuilt server's asset hashes are never stale),
`/api/state` is a one-shot `MonitorSnapshot` JSON, `/ws` upgrades to the delta
feed, and everything else falls through to `ServeDir` (a missing asset 404s
rather than serving HTML for the wrong MIME type).

## Parser public surface (`parser/src/lib.rs`)

- `parse_line(&str) -> ParsedLine` (`:1073`): classify one Nix stderr line into
  an `Event(NixEvent)`, `Plain { text }`, or `ParseError`. Events are tagged by
  `action`: `Start` / `Stop` / `Result` / `Message` / `Unknown`
  (`:115`), with `ActivityResult` variants for build-log lines, phases, progress,
  expected counts, fetch status, and `FileLinked` store-optimisation events
  (`:172`).
- `MonitorState` (`:276`): the accumulating state machine. `new(command)`,
  `apply_line` / `apply_parsed_line`, `snapshot() -> MonitorSnapshot`,
  `drain_deltas() -> Vec<Delta>`, `finish(exit_code)`, plus out-of-band setters
  `record_closure`, `set_daemon`, `set_activity_size`.
- `MonitorSnapshot` (`:958`) and `Delta` (`:248`): the seed-once / stream-deltas
  wire model the browser consumes. `BuildNode`/`BuildStatus` (`:1025`),
  `ActivityNode` (`:991`), `LogEntry` (`:1058`), `DerivationEdge` (`:984`),
  `OptimiseStats` (`:229`), `DaemonInfo` (`daemon::DaemonInfo`, including
  daemon syscall rates and hot paths).
- `daemon` module (`parser/src/daemon.rs`): `OpClass::classify`,
  `parse_fs_usage_line` (macOS), `parse_strace_line` (Linux), and the rolling
  `DaemonTrace` -> `DaemonInfo` aggregator.

## Build and packaging

- `parser/package.nix`: `inRustWorkspace`, `passthruTests`; library crate, no
  flake output.
- `server/default.nix`: builds the Svelte UI with `ix.buildSvelteSite`, selects
  the `nix-web-monitor` binary via `ix.cargoUnit.selectBinaryWithTests`
  (`:24`), then wraps it (`makeBinaryWrapper`) to set `NIX_WEB_MONITOR_SITE_DIR`
  to the bundled site and stamp `IX_BUILD_REV`/`IX_BUILD_EPOCH` for
  [build-version](../build-version/overview.md) (`:59`). Deliberately no PATH
  wrapping for `nix` (`:48`).
- Flake output / main program: `nix-web-monitor` (`server/package.nix`,
  `flake = true`). Run as `nix run .#nix-web-monitor -- build .#ix`.

## Dependencies

Parser: `serde`(+json), `snafu`, `strip-ansi-escapes`. Server: `axum` (+ws),
`tokio`, `tower-http`, `bytes`, `rmp-serde` (msgpack deltas), `ignore` (gitignore
walk for copy-size), `clap`, and the parser + `build-version` crates.
