# fff

`packages/fff` packages [fff](https://github.com/dmtrKovalenko/fff), a fast
file-search toolkit for humans and AI agents, as a repo Nix package. It is a
third-party Rust tool, not a workspace member: built from a pinned flake source
input (`fff-src`, `github:dmtrKovalenko/fff/v0.9.1`) with
`rustPlatform.buildRustPackage`, the external-Rust-tool house style
(`default.nix:9-11`). It is independent of the corpus stack (`search-core`,
the Mixedbread store, the source adapters); it is a local file-search toolkit.

## Artifacts

One build emits two artifacts (`default.nix:13-17`):

- `bin/fff-mcp`: the CLI / MCP server over fff's in-memory file index. It is the
  package `mainProgram` (`nix run .#fff`, `default.nix:80`).
- `lib/libfff_c.{so,dylib}`: the stable C ABI (crate `fff-c`), a cdylib the
  `mcp` package loads via ctypes to expose `import fff` in notebook sessions.

`cargoBuildFlags`/`cargoTestFlags` scope the build to `fff-mcp` and `fff-c` in
one cargo invocation (sharing the dependency compile) rather than the whole
upstream workspace, which keeps out `fff-nvim`'s mlua/Lua build
(`default.nix:48-59`). `buildNoDefaultFeatures = true` skips the upstream `zlob`
feature, which shells out to a system Zig at build time; without it the crate
falls back to the pure-Rust globset matcher (`default.nix:44-47`,
`buildNoDefaultFeatures` at `:60`). `postInstall` copies the unhashed
`libfff_c` cdylib into `lib/` for both Linux `.so` and macOS `.dylib`
(`default.nix:65-74`).

## Build wiring

`cargoLock.lockFile` reads fff's committed pure-crates.io `Cargo.lock` straight
from the source, so a rev bump carries the dependency set with no coarse
`cargoHash` to refresh (`default.nix:27-30`). `nativeBuildInputs` are `cmake` and
`pkg-config` for libgit2-sys (vendored libgit2) and lmdb-master-sys
(`default.nix:34-39`).

`package.nix` sets `packageSet`, `flake`, and `overlay = true`, so fff is
`pkgs.fff` in the repo package set, the `fff` flake output, and available to
other repo packages through the nixpkgs overlay: the `mcp` package takes
`pkgs.fff` as an input and bundles the `fff-c` cdylib for its ctypes-backed
`import fff` (`package.nix:3-10`). It builds on Linux and macOS
(`meta.platforms = unix`, `default.nix:81`).

## Claude Code `@` completion (`packages/agent/fff-suggest`)

fff also backs Claude Code's `@`-mention file completion, replacing the CLI's
built-in index. Claude Code exposes a statusLine-shaped custom completer via the
`fileSuggestion` setting: it runs a command per keystroke (5s budget, cwd = the
project dir), passes `{ query, … }` on stdin, and uses each non-empty stdout line
as a suggestion **in the returned order** (no re-ranking) — so fff owns the
ranking.

That command is the per-keystroke client half of `packages/agent/fff-suggest`, a
repo-owned rust workspace crate with one binary and two subcommands:

- `fff-suggest query` — the tiny native client wired into `fileSuggestion`. It
  reads the query, round-trips it to the daemon over a unix socket, prints the
  ranked relative paths, and **fails open** (any error exits 0 with no output, so
  Claude never hangs on its budget).
- `fff-suggest serve <root>` — the resident per-project daemon. It `dlopen`s the
  same `libfff_c` the kernel uses (`IX_FFF_LIB`, baked by the `fff-suggest`
  wrapper), holds one frecency-ranked, file-watched index over `<root>`, and
  answers over the socket until it sits idle (`IX_FFF_SUGGEST_IDLE_MS`, default
  10 min). The client auto-starts it detached on the first `@` in a project;
  every keystroke after that is a warm socket round-trip with **no Python on the
  hot path** and no re-index.

The socket lives at `<runtime-dir>/ix-fff-suggest/<hash(root)>.sock`. The wiring
is gated on the `fff-suggest` sibling in `packages/agent/claude-code` (like the
`search`/`mcp` siblings, only the flake package set provides it), and an
install-check there guards the `fileSuggestion.command` shape against a CLI bump.
