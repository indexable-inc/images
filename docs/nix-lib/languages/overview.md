# lib/languages: per-language toolchains

`lib/languages/` selects compilers, interpreters, runtimes, and dev tools for
21 languages from a caller's `pkgs`. Each language is one `.nix` file (Java is a
directory) returning an attrset of helper functions, exposed as
`ix.languages.<lang>.<fn>`. `lib/default.nix:128-146` imports each, passing
`errors` (and `rust-overlay` for Rust, `lib` for Java).

These are selectors, not builders: they return a package (a JDK, a Go toolchain,
a compiler) for a module to drop into `environment.systemPackages` or feed to a
builder. For building Rust crates see [rust](../rust/overview.md); for building
JS/Python/Go/Gradle/Zig projects see [build-helpers](../build-helpers/overview.md).

## The common pattern

Every helper is curried `pkgs: args:` and returns a package
(`packages/registry.nix`-style consumers pass their own `pkgs` so the result
comes from the image's evaluation, not the lib default):

- A **version map** `xxxFor pkgs` lists each supported version string against the
  nixpkgs attribute (e.g. `lib/languages/go.nix:13-18`,
  `lib/languages/python.nix:9-15`). Listing them explicitly means an unknown
  version throws with the supported set instead of `attribute missing` deep in
  eval; bump the top entry when nixpkgs ships a newer release.
- The main selector requires `version` via `errors.requireArg` and looks it up
  via `errors.requireAttr` (`lib/languages/go.nix:45-58`). A floating default is
  avoided on purpose: it would silently retarget every consumer on a nixpkgs
  bump (`lib/languages/python.nix:25-28`).
- `version = "latest"` follows the channel default (`pkgs.go`, `pkgs.ghc`, ...)
  where the language file provides that key.
- Validation goes through [`ix.errors`](../util/overview.md): `assertEnum` for
  enum fields (Rust channel/profile, `lib/languages/rust.nix:115-125`),
  `requireArg`/`requireAttr` for required selectors.
- Secondary tools (`languageServer`, `cmake`, `delve`, `sbt`, ...) are usually
  `pkgs: _: pkgs.<tool>`, ignoring args and returning a floating package
  (`lib/languages/go.nix:82`, `lib/languages/cpp.nix:142`).

`lib/languages/jvm-defaults.nix` is the one shared constant: the default JVM
major (`"25"`) read by `java`, `scala`, the overlay's `ixDefaultJre`
(`lib/overlay.nix:62`), and the JVM service modules so they agree without
pinning.

## Per-language surface

| language | file | main selector(s) | versions / notes |
| --- | --- | --- | --- |
| rust | `rust.nix` | `toolchain pkgs { channel, version, components?, targets?, profile? }` | rust-overlay `fromRustupToolchain`; channel `stable\|beta\|nightly`, version `latest`/semver/date |
| python | `python.nix` | `interpreter pkgs { version }` | 3.10-3.14 |
| go | `go.nix` | `toolchain`, `delve`, `languageServer` | latest, 1.23, 1.25, 1.26 |
| java | `java/default.nix` | `jdk pkgs { version, distribution }`, `maven`, `gradle`, `languageServer`, `yourkit` | versions 8-25; distributions openjdk/temurin/corretto/zulu; gradle 7/8/9 |
| javascript | `javascript.nix` | `node pkgs { version }`, `bun`, `deno`, `typescript`, `languageServer` | node latest, 20, 22, 24, 25 |
| scala | `scala.nix` | `compiler pkgs { version, jdk? }`, `sbt`, `mill`, `languageServer` | Scala 2 / 3; JDK from jvm-defaults |
| kotlin | `kotlin.nix` | `compiler pkgs { target }`, `languageServer`, `repl` | target `jvm`/`native` |
| haskell | `haskell.nix` | `compiler pkgs { version }`, `cabal`, `stack`, `languageServer` | latest, 9.6-9.14 |
| ocaml | `ocaml.nix` | `compiler pkgs { version }`, `dune`, `opam`, `ocamlformat`, `utop`, `languageServer` | latest, 4.14, 5.1-5.4 |
| elixir | `elixir.nix` | `toolchain pkgs { version }`, `languageServer` | latest, 1.15-1.19 |
| erlang | `erlang.nix` | `toolchain pkgs { version }`, `rebar3`, `languageServer` | latest, 26, 27, 28 |
| cpp | `cpp.nix` | `compiler pkgs { vendor, version }`, `cmake`, `ninja`, `meson`, `make`, `languageServer` | gcc 9-15, clang 16-22 |
| zig | `zig.nix` | `toolchain pkgs { version }`, `languageServer` | latest, 0.12-0.16 |
| dhall | `dhall.nix` | `interpreter`, `json`, `nix`, `languageServer` | floating; no version map |
| gleam | `gleam.nix` | `compiler` | floating (skips network test); pair with erlang/node |
| idris | `idris.nix` | `compiler` | floating `pkgs.idris2` |
| futhark | `futhark.nix` | `compiler` | floating `pkgs.futhark` |

Java is a directory: `java/default.nix` holds the JDK/maven/gradle selectors and
re-exports `yourkit = import ./yourkit.nix` (`lib/languages/java/default.nix:246`),
the YourKit profiler agent options/flags module (an opt-in unfree agent,
`lib/languages/java/yourkit.nix`). `jvm-defaults.nix` is the shared JDK major.

## Examples

```nix
{ pkgs, ix, ... }:
let
  rust = ix.languages.rust.toolchain pkgs { channel = "nightly"; version = "2025-12-01"; };
  jdk  = ix.languages.java.jdk pkgs { version = "21"; distribution = "temurin"; };
  go   = ix.languages.go.toolchain pkgs { version = "1.25"; };
in {
  environment.systemPackages = [ rust jdk go ];
}
```

The full attrset is also on `ix.languages` directly (e.g. for builders:
`lib/rust/tooling.nix:30` uses `languages.rust.toolchain`,
`lib/build/go-unit.nix` takes `go = languages.go`).
