---
name: dependency-intake
disclosure: progressive
description: "Adding external dependencies: owners, fetchers, real hashes, generated catalogs, updateScripts, impure boundaries. Use when adding or updating a dependency, fetcher, or prebuilt binary."
---

## Dependency intake

Every external input needs an owner that can update it predictably. Prefer
ecosystem lockfiles, flake inputs for real flake-level tools, repo manifests
consumed by updaters, or narrow `pkgs.*` fetchers when no better owner exists.

The human workflow is: edit the source requirement or manifest, run the owning
updater, inspect the generated diff, and commit the source and generated
hash-bearing output together.

Use the most specific `pkgs.*` fetcher for the source: `fetchurl` for opaque
single files, forge fetchers for forge snapshots, `fetchgit` for raw git refs,
`fetchzip` for archives that must unpack, and ecosystem fetchers when one
exists. Avoid `builtins.fetch*` in tracked Nix files because those fetch during
eval and do not substitute like fixed-output derivations.

Tracked Nix files should never contain fake hash helpers or placeholder hashes.
Materialize real SRI hashes with the owning updater, lock command,
`nix flake update`, or a checked prefetch command before committing.

Use `__impure` only for explicit dependency-discovery or prefetch derivations
that are turned into a checked hash-bearing artifact before product builds
consume them. Keep the impure boundary named next to the updater or generated
lock output that makes later builds pure.

Generated catalogs are build inputs, not hand-edited source. If a generated file
is wrong, change the manifest or generator that owns it.

A prebuilt-binary package pins its version and per-platform hashes in a generated
`manifest.json` read with `lib.importJSON` and refreshed by a
`passthru.updateScript`; bump by running the updater, never by hand-editing the
hashes. When upstream signs its release manifest, the updater verifies that
signature against a pinned key and fails closed before writing hashes. See
[`packages/claude-code`](packages/claude-code) for the worked shape:
`nix run .#claude-code.updateScript -- <version>`.

Keep binary and generated artifacts near the owner that can explain and refresh
them. Use small manifests for curated sets, generated catalogs for URLs and
hashes, and metadata catalogs for search or browsing surfaces.

Repository examples should consume those shared surfaces. Repeating URLs and
hashes in examples creates second owners with no update story.

### Packaging external Rust CLI/TUI tools

Build a standalone third-party Rust binary with `pkgs.rustPlatform.buildRustPackage`
in `packages/<name>/default.nix`, paired with a `package.nix` that carries `id`
and the systems it builds on. Reserve [`ix.cargoUnit`](lib/cargo-unit.nix) for
crates inside the shared workspace: its per-unit graph, nightly `RUSTC_BOOTSTRAP`,
and workspace policy cost more than one outside tool returns, and its
content-addressed dedup is off on macOS.

Pin the source in the derivation with `pkgs.fetchFromGitHub` at a git rev. A
flake input (the [`packages/llm-clippy`](packages/llm-clippy) shape) earns its
top-level slot only when the source is shared across consumers or wants
`nix flake update`. A one-off tool keeps its owner local in `default.nix`, and
the package registry still discovers the directory.

When upstream commits a `Cargo.lock`, read it with
`cargoLock.lockFile = src + "/Cargo.lock"` so a rev bump carries the dependency
set. Reach for `cargoHash` only when there is no committed lock. `cargoSha256`
is banned.

Two quirks recur in macOS Rust tools, each with one fix:
- A crate that runs `bindgen` for FFI bindings needs `rustPlatform.bindgenHook`
  in `nativeBuildInputs` for libclang.
- A crate that reads VCS state at build time (`git_version!()`, `vergen`) fails
  in the sandbox because the fetched tarball has no `.git`. Patch the call out in
  `postPatch`, e.g. `--replace-fail "git_version!()" 'env!("CARGO_PKG_VERSION")'`.

Set `strictDeps = true`, a typed `meta.license`, `meta.mainProgram`, and
`meta.platforms`. A platform-bound tool gates both `meta.platforms` and the
`package.nix` systems so `nix flake check` does not force it off-platform.
[`packages/launchk`](packages/launchk) (Darwin-only) is the reference.
