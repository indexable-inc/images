# SDKs

The language SDKs for ix. Everything under `sdk/` is a client library for the
hosted ix service (the sandbox/microVM API at `https://api.ix.dev`), shipped in
three languages, plus the Nix packaging that turns the precompiled Python
bindings into a `pkgs.ix-sdk-python` for in-repo consumers. These are public,
source-available wrappers: the load-bearing cores are compiled in the private
`indexable-inc/ix` monorepo and distributed as prebuilt artifacts (a Rust rlib,
a Python wheel, a napi `.node` addon, a wasm bundle) fetched from a public R2
bucket. The `sdk/` tree carries the wrapper/glue layer and metadata stubs only,
never the private source.

Read this page first, then the per-language component pages it links.

## Licensing (read before reusing)

Everything under `sdk/` is governed by `sdk/LICENSE` (the Indexable SDK
License), which supersedes the repo-root MIT for this directory and the compiled
components the SDK fetches (`sdk/README.md:6-14`). It is source-available, not
open source: you may build apps that access the hosted ix service, but not
reverse-engineer, modify, redistribute, or build a competing service. The Rust
crates set `license-file` (not an SPDX id) and the Python project carries the
`License :: Other/Proprietary License` classifier so crawlers surface the real
terms (`sdk/rust/crates/ix-sdk/Cargo.toml:5-9`,
`sdk/python/pyproject.toml:14-15`).

## Units

| unit | kind | role |
| --- | --- | --- |
| `sdk/rust` (`ix-sdk`) | Rust workspace member | public Rust crate; re-exports the wire-protocol surface, links the prebuilt `ix-sdk-wire` rlib. See [rust](rust/overview.md). |
| `sdk/rust/vendor/ix-sdk-wire` | Rust crate (metadata stub) | self-contained C-ABI wire types; here only as a metadata-faithful stub, the real rlib is fetched from R2. See [rust](rust/overview.md). |
| `sdk/python` (`ix_sdk`) | Python package | async client wrapper over the native `_ix_sdk` PyO3 extension. See [python](python/overview.md). |
| `sdk/typescript` (`@indexable/sdk`) | TypeScript package | thin wrapper dispatching to a napi `.node` addon (Node/Bun) or a wasm bundle (browser). See [typescript](typescript/overview.md). |
| `packages/ix-sdk-python` | Nix package | fetches the prebuilt `ix_sdk` wheel from R2, wraps it as `pkgs.ix-sdk-python`. See [packaging](packaging/overview.md). |

The Rust SDK in this tree is the wire-protocol layer (boundary types + a link
proof), not a full client. The Python and TypeScript packages are the full
clients (`Client`, `Branch`, `Sandbox`, ...); their cores are the compiled
`_ix_sdk` / `ix_sdk.node` / wasm binaries built from `crates/ix/sdk-py`,
`crates/ix/sdk-ts`, `crates/ix/sdk-wasm` in the private repo.

## How it fits together

```
your app (Rust / Python / TS / browser)
  -> language wrapper (sdk/rust, sdk/python, sdk/typescript)
     -> precompiled core (ix-sdk-wire rlib / _ix_sdk wheel / ix_sdk.node / wasm)
        -> hosted ix service at base_url (default https://api.ix.dev)
           Client -> Branch (a microVM) -> exec/fs/secrets/fork/snapshot/...
```

- **One service, one auth model.** All three SDKs talk to the same hosted ix
  API. Auth is an API token (a bearer token), passed to the client constructor
  or resolved from the environment (`IX_TOKEN`, then `IX_API_KEY` for the TS
  `Sandbox`; `sdk/typescript/src/index.ts:1421`). The base URL defaults to
  `https://api.ix.dev` (`sdk/typescript/src/index.ts:1428`,
  `sdk/typescript/README.md:27`); the Python/native client resolves its default
  internally and exposes it via `Client.base_url`
  (`sdk/python/ix_sdk/__init__.py:886-888`).
- **Shared object model.** The same nouns recur across languages: `Client`
  (auth + account/fleet ops), `Branch` (a running microVM, also surfaced as
  `VM`/`Sandbox`), `Commit`/`Snapshot` (a paused VM image you can branch or
  fork), `FsHandle`, `SecretsHandle`, `StreamConnection`, `ExecResult`,
  `RegionInfo`. The Python `_ix_sdk.pyi` and the TS `native.d.ts` are
  hand-maintained mirrors of the same Rust binding surface
  (`sdk/python/ix_sdk/_ix_sdk.pyi:3`, `sdk/typescript/src/native.d.ts:6-9`).
- **Prebuilt, not built here.** Each package fetches its compiled core from the
  public R2 bucket by SRI hash; the source-available tree never compiles the
  private core. The Rust workspace injects the R2 rlib over a stub
  (`sdk/rust/default.nix:69-93`); `packages/ix-sdk-python` fetches the R2 wheel
  (`packages/ix-sdk-python/default.nix:24-33`); the TS package loads
  `../native/ix_sdk.node` or `../dist/ix_sdk.js`, which `build-native.sh`
  populates (`sdk/typescript/src/index.ts:53-73`).
- **Default region** is `us-west-1` (`sdk/python/ix_sdk/__init__.py:176`,
  `sdk/typescript/src/index.ts:81-83`), overridable per call or via `IX_REGION`.

## Invariants

- **Source-available wrapper, private core.** No private ix source ships under
  `sdk/`. The Rust build even asserts the from-source stub unit is excluded from
  the consumer closure (`sdk/rust/default.nix:238-254`); the wire stub source is
  never compiled (`sdk/rust/vendor/ix-sdk-wire/src/lib.rs:3-6`).
- **Stub fidelity is load-bearing.** The `ix-sdk-wire` stub's Cargo metadata
  (name, version, edition, deps, profile, lints, toolchain) must match the real
  crate exactly, because cargo-unit hashes those and not source bytes; a mismatch
  makes the R2 rlib un-injectable (`sdk/rust/vendor/ix-sdk-wire/Cargo.toml:8-22`,
  `sdk/rust/default.nix:56-63`).
- **Type stubs track Rust by hand.** `_ix_sdk.pyi` and `native.d.ts` are
  hand-synced to `crates/ix/sdk-py` / `sdk-ts`; add a Rust method, add it to the
  stub (`sdk/python/ix_sdk/_ix_sdk.pyi:3-8`, `sdk/typescript/src/native.d.ts:6-9`).
- **Async everywhere.** Every network call is async: `async def` in Python,
  `Promise` in TS. Resources clean up on scope exit (`async with` /
  `await using` via `__aexit__` / `Symbol.asyncDispose`).

## Glossary

- **Branch**: a running microVM instance; the primary handle for exec, fs,
  secrets, fork, snapshot. Also surfaced as `VM` (Python) and wrapped by
  `Sandbox` (TS).
- **Commit / Snapshot**: a paused, persisted VM image. `branch()` boots a fresh
  branch from it; `fork()` makes a divergent copy. Python calls it `Snapshot`,
  the native layer calls it `Commit`.
- **Sandbox / Repl** (TS): opinionated high-level surface; `Sandbox` is a
  Client+Branch convenience, `Repl` is a stateful interpreter session over a PTY
  shell (`sdk/typescript/src/index.ts:1182-1376`).
- **wire protocol**: the self-contained types crossing the SDK C-ABI boundary,
  defined by `ix-sdk-wire` (`IxBuf`, `IxError`, error codes;
  `sdk/rust/crates/ix-sdk/src/lib.rs:12-14`).
- **cargo-unit / extraUnits**: the `nix-cargo-unit` build system; `extraUnits`
  is the seam that injects a prebuilt library unit over a from-source one
  (`sdk/rust/default.nix:174-184`).
- **R2**: the public Cloudflare R2 bucket hosting the prebuilt rlib/rmeta and
  wheels (`pub-559bccbc8be94bed84821cb943b580f3.r2.dev`).
- **abi3 wheel**: the stable-ABI CPython wheel (`cp313-abi3`) the Nix package
  fetches (`packages/ix-sdk-python/default.nix:26`).
- **napi-rs / wasm-bindgen**: the two binding generators behind the TS package;
  napi produces the Node/Bun `.node` addon, wasm-bindgen the browser bundle.

## Components

| component | page | what |
| --- | --- | --- |
| rust | [rust/overview.md](rust/overview.md) | `ix-sdk` + the `ix-sdk-wire` stub; wire types and the prebuilt-rlib link proof |
| python | [python/overview.md](python/overview.md) | `ix_sdk` async client over the native `_ix_sdk` PyO3 extension |
| typescript | [typescript/overview.md](typescript/overview.md) | `@indexable/sdk`; Sandbox/Repl + Client/Branch over napi or wasm |
| packaging | [packaging/overview.md](packaging/overview.md) | `packages/ix-sdk-python`: the R2 wheel wrapped as `pkgs.ix-sdk-python` |
