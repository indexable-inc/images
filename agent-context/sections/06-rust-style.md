---
name: rust-style
disclosure: progressive
description: "Rust house style for repo-owned crates: edition 2024, naming, module layout, unsafe validation (Miri/loom/shuttle), mutation testing, fuzzing. Use when writing or reviewing Rust."
---

## Rust style

Repo-owned crates, fixtures, examples, and generated manifests use Rust edition
2024. Fix compatibility issues directly and document unavoidable upstream
blockers next to the exception.

Prefer names that preserve the concept's path. Local aliases may shorten noisy
source paths only when the shape remains visible at the call site. Keep singular
names for single values and plural names for bags of constructors, helpers, or
registry entries.

Use local type annotations when they make the data shape clearer. Keep turbofish
for expression-local cases where an intermediate binding would add noise.

Use normal module layout. Move files so `mod` declarations follow the filesystem
instead of using `#[path = ...]`.

Avoid anonymous tuple-shaped domain data once a value crosses a function
boundary. Prefer named structs or full paths for values that carry real meaning.

Use blank lines as paragraph breaks inside functions: set up, act, then validate
or return. Keep tightly coupled statements together.

When parsing, normalizing, serializing, traversing graphs, handling archives, or
speaking protocols, start from a maintained crate. Hand-written logic is for the
thin glue around that crate unless the dependency boundary is measurably worse.

Validate `unsafe` Rust with runtime checks before trusting normal tests. Run
Miri where it works; for blocks Miri rejects because they need FFI, platform
syscalls, or real native execution, run [`cargo-careful`](https://github.com/RalfJung/cargo-careful)
with `cargo +nightly careful test -p <crate>`. cargo-careful exercises code
against a debug-assertion standard library and surfaces some unsafe-precondition
and stdlib-invariant breakage, but it does not model aliasing, uninitialized
reads, or data races, so it complements Miri rather than replacing it.

Use [`loom`](https://docs.rs/loom/latest/loom/) for small deterministic
concurrency primitives whose state fits inside modeled threads, atomics, and
`std::sync` replacements. Use [`shuttle`](https://docs.rs/shuttle/latest/shuttle/)
for larger randomized scheduler tests, especially Tokio-shaped workflows; skip
both when the test would mainly prove a dependency's lock, channel, or runtime
works instead of a repo-owned invariant.

When auditing a crate with deterministic, fast tests, run
[`cargo-mutants`](https://mutants.rs/) with
`nix shell nixpkgs#cargo-mutants -c cargo mutants --package <name>` to surface
behavior that coverage cannot prove protected. Let the default copy-to-`target`
mode hold; `--in-place` is faster but leaves the source tree dirty on interrupt
or panic, so reserve it for disposable checkouts. Treat surviving mutants as
candidates for tighter assertions, equivalent-mutant write-offs, or
unreachable-by-test code, and keep cargo-mutants a package-owner tool rather
than a CI gate: equivalent mutants need human judgment, runtime scales with
mutant count, and a survivor is a prompt to look, not a regression to block.

Fuzz Rust surfaces that read untrusted bytes: parsers, codecs, deserializers,
protocol handlers, archive readers, and unsafe or FFI-adjacent input edges.
Scaffold with `cargo fuzz init` so targets land in
`packages/<crate>/fuzz/fuzz_targets/<name>.rs`; the fuzz crate keeps its own
`[workspace]` table so it stays off the main `cargo --workspace` graph. Commit
hand-picked seeds under `fuzz/seeds/<target>/`, gitignore `fuzz/corpus/`, and
minimize crashes with `cargo fuzz cmin <target>` or
`cargo fuzz tmin <target> <path>` before committing the reduced input as a
regression seed. `packages/minecraft/nbt/fuzz/` is the worked example; see
[the cargo-fuzz book](https://rust-fuzz.github.io/book/cargo-fuzz.html) for
the libFuzzer flag surface.
