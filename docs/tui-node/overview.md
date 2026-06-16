# tui-node

`packages/tui-node` is the Node.js (N-API) binding for [tui](../tui/overview.md):
spawn and drive PTY-backed programs (vim, a REPL, a shell) from Node with full
VT100 emulation, plus the in-process web dashboard. It is a thin binding built
with [napi-rs](https://napi.rs/); the `tui` crate owns every behavior. npm name
`@indexable/tui`, native addon `tui_node.node` (`packages/tui-node/README.md:12`).

## What it is

- Rust crate `tui-node` (`Cargo.toml:2`), `crate-type = ["cdylib"]`
  (`Cargo.toml:9`), depending on `tui` with the `dashboard` feature
  (`Cargo.toml:17`) and `napi`/`napi-derive`.
- A hand-written JS wrapper (`npm/index.js`) plus TypeScript types
  (`npm/index.d.ts`) layered over the addon.
- A `nix build .#tui-node` output that emits an npm package tree
  (`package.json`, `index.js`, `index.d.ts`, `native/tui_node.node`). Linux only
  (`package.nix:8`), like [tui-py](../tui-py/overview.md), because the shared
  cargo-unit graph does not thread macOS's `-undefined dynamic_lookup` through
  the link step; for macOS dev use plain `cargo build -p tui-node`
  (`README.md:64-69`).

## Public surface

The native addon exports three items (`src/lib.rs`); the JS wrapper re-exports
them and adds two pure-JS helpers (`npm/index.js:72-76`).

`Tui` (`src/lib.rs:74`), one spawned process:

- `new(command, args?, options?)` (`src/lib.rs:83`): spawn on a fresh PTY and
  track it. `SpawnOptions { rows?, cols?, scrollbackLines? }` (`src/lib.rs:60`),
  unset fields fall back to 80x24 / 10,000.
- Static `Tui.listAll()` (`src/lib.rs:108`).
- Synchronous getters: `id`, `command`, `args`, `rows`, `cols`,
  `scrollbackLimit` (`src/lib.rs:118-146`); instant state `isAlive()`,
  `exitCode()` (`src/lib.rs:191,197`).
- Async I/O (each returns a `Promise`, runs on the tui actor, never blocks the
  event loop): `write(data)`, `readViewport()`, `readScrollback()`, `readFull()`
  (`{ scrollback, viewport }`), `readBlocking(timeoutMs)` (`src/lib.rs:152-185`).
- Lifecycle: `wait()` (resolves to the exit code, `null` if signalled),
  `kill()` (SIGKILL), `resize(rows, cols)` (delivers `SIGWINCH`),
  `close()` (force-kill and drop from `listAll` and the dashboard,
  `src/lib.rs:207-234`).

`serve(host?, port?, pollMs?) -> Promise<Dashboard>` (`src/lib.rs:279`): start
the in-process Loro-backed web dashboard for every live `Tui`. `host` must be an
IP literal (a hostname is not resolved, `src/lib.rs:289`); `port = 0` binds an
ephemeral port read back from `Dashboard.url`. `Dashboard` (`src/lib.rs:239`)
exposes `url`, `addr`, and `stop()`. The server, the CRDT document, and the SSE
stream are all in Rust ([dashboard-core](../dashboard-core/overview.md));
the browser imports updates with `loro-crdt`.

JS-only helpers (`npm/index.js`):

- `Key` (`index.js:12`): frozen ANSI keystroke constants (`ENTER`, `ESC`, arrows,
  `PAGE_UP`, `CTRL_C`, ...) plus `Key.ctrl(letter)` and `Key.alt(letter)`. They
  are plain strings, so they concatenate with literal text and pass to `write`.
- `waitFor(tui, pattern, { timeoutMs, pollMs })` (`index.js:51`): poll the
  viewport until a substring, RegExp, or predicate matches, returning the lines;
  throws on timeout.

## Wiring

The addon holds a single process-wide `tui::TuiManager` in a `OnceLock`
(`src/lib.rs:38`); every `Tui` is a handle into it, mirroring the Python binding.
JS numbers cross in as `u32`; `narrow_u16` rejects out-of-range rows/cols/ports
rather than wrapping (`src/lib.rs:52`). Errors become rejected Promises via
`Error::from_reason` (`src/lib.rs:46`).

## Build details

`package.nix` sets `id = "tui-node"`, `inRustWorkspace = true`, and restricts the
flake/package set to `x86_64-linux`/`aarch64-linux` (`package.nix:8-15`). The Nix
build (`default.nix`) does not run `napi build` or `node-gyp`: it takes the
cdylib already produced by the shared workspace graph
(`ix.rustWorkspace.units.libraries.tui_node`, `default.nix:10`), renames it to
`native/tui_node.node`, strips the build-time rpath and toolchain references with
`patchelf`/`remove-references-to` so the artifact is not pinned to a store path
(`default.nix:67-75`), and stamps `package.json`'s `cpu`/`libc` for the build
arch (`default.nix:62`). `package.json` declares `"os": ["linux"]` and
`engines.node >= 20` (`npm/package.json`).
