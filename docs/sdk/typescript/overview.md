# TypeScript SDK

`sdk/typescript` is the `@indexable/sdk` package (`package.json:2`): a thin
TypeScript wrapper that runs on Node, Bun, and browsers. It dispatches at
runtime to one of two precompiled cores - a napi-rs `.node` addon on Node/Bun,
or a wasm-bindgen bundle over WebTransport in browsers - neither of which is in
this tree. Both are built from the private `crates/ix/sdk-ts` (napi) and
`crates/ix/sdk-wasm` (wasm); `src/native.d.ts` is the hand-maintained type
declaration for the napi binding, mirroring the wasm `.d.ts`
(`native.d.ts:1-10`).

See [common](../common.md) for the cross-SDK model, auth, and licensing.

## Package metadata

- Name `@indexable/sdk`, version `0.3.0`, ESM (`type: module`),
  `license: SEE LICENSE IN LICENSE` (`package.json:2-5`).
- Entry is the TS source itself: `main`/`types` -> `./src/index.ts`
  (`package.json:6-7`). `exports` also surface the native addon
  (`./native/ix_sdk.node`) and the wasm bundle (`./dist/ix_sdk.js`)
  (`package.json:8-12`). Published files: `src`, `native`, `dist`, `LICENSE`
  (`package.json:13-18`). `engines.node >= 22` (`package.json:19-21`).
- The wrapper is "intentionally thin": core behavior lives in the Rust SDK;
  this layer adds `await using`, async iterators, overloads, and typed options
  (`README.md:38-40`).

## Runtime dispatch (`src/index.ts:32-83`)

`isNodeOrBun()` checks for a global `Bun` or `process.versions.node`
(`index.ts:34-43`). The `Client` constructor branches on it
(`index.ts:871-880`):

- **Node / Bun**: `loadNative()` resolves `../native/ix_sdk.node` and loads it
  through `createRequire` (imported via a runtime-stringified `'node:module'`
  specifier so browser bundlers do not pull Node types). The `.node` file is
  populated by `build-native.sh` from the `libix_sdk_ts` cdylib
  (`index.ts:53-73`).
- **Browser**: `ensureSdkReady()` runs the wasm-bindgen `init()` once, then
  constructs the `WasmClient` over native WebTransport (`index.ts:589-598`,
  `876-879`). Node/Bun never boot the wasm path; browsers never see the native
  path (`index.ts:9-12`).

Inner handles are typed as unions of the wasm and napi classes, which share
identical signatures, so each wrapper method delegates straight through
(`index.ts:600-614`).

## Public surface

### `Client` (`src/index.ts:868-1033`)

Constructed with `ClientOptions { token, baseUrl }` (`index.ts:86-91`). All
methods return `Promise`. The full surface mirrors the Python `Client` plus the
richer billing/usage and API-token surface that the napi binding exposes
(`native.d.ts:564-591`):

- `get(id)`, `getByName(name)`, `branches()`, `regions()`, `currentUsername()`
  (`index.ts:886-906`).
- `create(...)` with two overloads (`(image, options, onProgress?)` and
  `(options, onProgress?)`); it normalizes args and passes `onProgress` through
  only when the inner `create` accepts it (the napi core does not yet;
  `index.ts:908-950`). `buildCommitFromOci(options)` returns a `Commit`
  (`index.ts:952-954`).
- Account/billing: `me()`, `billingStatus()`, `listApiTokens()`,
  `createApiToken(options)`, `createTopUpSession(options)`,
  `usageEvents(options)`, `usageSummary(options)`, `revokeApiToken(id)`
  (`index.ts:956-994`).
- Volumes/previews/observability: `getVolume`, `listVolumes`,
  `listVolumeSnapshots`, `createPreview`, `listPreviews`, `stopPreview`,
  `queryLogs`, `searchTraces` (`index.ts:996-1032`).

### `Branch` (`src/index.ts:744-864`)

Wraps the inner branch. Surface: `id`, `fs()`, `secrets()`, `info()`,
`delete()` (also `Symbol.asyncDispose`), `start()`, `restart()`, `pause()`,
`commit()`, `runtimeStatus()` / `runtimeStatusObservation()`, `metrics()`,
`fork(name?)`, `migrate(targetNodeId?)`, `cancelMigration(id)`, `migration()`,
`exec`/`execChecked`/`bash`/`bashChecked` (options objects), `spawn`,
`logs`/`log`, `consoleConnect()`, `portForward(port)`, `shell(options)`,
`shellList()`, `subscribeStatus()` (`index.ts:747-863`). The PTY `shell` /
`ShellSession` / `shellList` surface has no Python equivalent
(`native.d.ts:507-562`).

### Handles and streams

- `FsHandle` (`index.ts:688-722`): `read`/`write` (options), `readBytes`,
  `readAllBytes`, `writeAllBytes`, `list`.
- `SecretsHandle` (`index.ts:726-740`): `set`/`delete`/`list`.
- `StreamConnection` (`index.ts:618-636`): `read`/`write`/`close` +
  `Symbol.asyncDispose`. `ShellSession` (`index.ts:640-666`) adds `resize`.
- `VmStatusStream` (`index.ts:670-684`): `next()` plus `Symbol.asyncIterator`,
  so `for await (const e of vm.subscribeStatus())` works.

### `Sandbox` (`src/index.ts:1283-1443`)

The opinionated high-level surface over `Client` + `Branch`. Static
constructors: `oci(image, options)`, `ubuntu(version)`, `python(version)`,
`node(version)`, `bun(version)`, and `attach(vmId, options)`
(`index.ts:1290-1343`). Instance methods: `exec(command, {cwd})` (fire-and-forget
subprocess), `repl(language, options)`, `read`/`write`/`readBytes`/`writeBytes`/
`list`, `fork(name?)`, `close()`, and `Symbol.asyncDispose`
(`index.ts:1357-1415`). `buildClient` resolves the token from `options.token`,
then `IX_TOKEN`, then `IX_API_KEY` (throwing if none), and the base URL from
`options.baseUrl`, `IX_API_BASE_URL`, else `https://api.ix.dev`
(`index.ts:1417-1430`). `resolveRegion` uses `options.region`, then `IX_REGION`,
then the first region the API returns (`index.ts:1432-1442`).

### `Repl` (`src/index.ts:1182-1281`)

A stateful interpreter session over a PTY `shell` (`mode: 'create'`,
`index.ts:1192-1213`). `exec(code)` frames the code with a random marker, writes
it to the PTY, and reads until a sentinel line, returning `{ output, exitCode }`
(`index.ts:1219-1255`). Bash uses a brace-group wrapper with `printf` of the
exit code; Python and JS use base64 `EXEC:<marker>:<payload>` lines consumed by
embedded harnesses: `PYTHON_HARNESS` keeps a persistent namespace dict so
variables survive across `exec` calls (`index.ts:1094-1121`), and `JS_HARNESS`
uses a shared `vm.createContext` for the same effect (`index.ts:1126-1150`).
`replCommand` maps `'bash' | 'python' | 'node' | 'bun' | 'typescript'` to the
launch argv (`index.ts:1152-1164`).

## Region

`Region` is both a type and a frozen runtime constant `{ UsWest1: 'us-west-1',
UsEast1: 'us-east-1' }`, kept in the TS layer (not a wasm call) so module-load
and SSR do not trigger wasm init (`index.ts:75-84`).

## Examples (`sdk/typescript/examples/`)

Run with Bun. `sandbox/python.ts` and `sandbox/bash.ts` show stateful REPLs and
that independent sessions do not share state; `sandbox/fork.ts` forks one
sandbox into 10 branches with a shared seed file; `minecraft/vanilla.ts` and
`minecraft/vanilla-ubuntu.ts` use the low-level `Client.create` with env vars to
boot game servers (`examples/sandbox/*.ts`, `examples/minecraft/*.ts`). Both
example dirs depend on `@indexable/sdk` via a workspace `*` version
(`examples/sandbox/package.json:5-6`).

## How it is built and wired

`sdk/typescript` has no `package.nix`/`default.nix`; the napi `.node` and wasm
`dist/` artifacts are produced in the private repo and dropped into `native/`
and `dist/` at publish time. There is no in-repo Nix consumer for the TS package
(unlike the Python wheel; see [packaging](../packaging/overview.md)).
