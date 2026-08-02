# Contributing

**This repository is a read-only projection.** `indexable-inc/index` is
published from a subtree of a private monorepo. Its `main` branch is replaced
wholesale by a bot on each sync, so nobody can push here and a pull request
cannot be merged — one opened against this repository is closed automatically,
with a pointer back to this file.

Read this section if you are looking at github.com/indexable-inc/index. If you
are reading the same file inside the monorepo at `index/`, everything below
"Local setup" still applies to you unchanged; the difference is only that your
changes are ordinary commits there, and reach this repository on the next sync.

## How to get a change made

Open an issue: <https://github.com/indexable-inc/index/issues>. Issues are read
and are the supported path. Security reports go through
[SECURITY.md](SECURITY.md) instead, not the issue tracker.

Two things are worth saying plainly rather than leaving you to infer them.

**A fix will not link back to your report.** The sync squashes each update into
a single commit titled `update <date>`, with no author, no message and no
revision from the monorepo — that is deliberate, because the commit metadata is
where internal detail leaks. So the change you asked for may well land, and
nothing in this repository's history will connect it to you. Say in the issue if
you would like to be credited and we will do it by hand.

**A pull request will be closed, not ignored.** Leaving it open would be worse:
the next sync overwrites `main` regardless, so the branch it targets stops
existing and the pull request is silently obliterated. Closing it immediately
with an explanation is the honest version of what happens either way. Nothing
about that is a judgement of the change — please do reopen the content as an
issue, patch and all.

## Local setup

Local verification is the gate that decides whether a change is safe to land, not CI. A single shared runner node serves the whole team, so routing routine validation through it overloads the node and slows everyone down; verify locally, then push. Run the repo lint before pushing:

```sh
nix run .#lint
```

It checks Nix formatting (alejandra), Statix, Deadnix, and the repo's astlog rules (`astlog-rules/nix.astlog` for Nix, `astlog-rules/rust.astlog` for the corpus/search Rust crates). CI runs the same derivation as a flake check, but treat that run as advisory signal rather than a landing gate.

There is no `devShells.default` to enter for routine work. Reach for the per-package shell when you need build dependencies for a specific artifact, e.g.

```sh
nix develop .#minestom-hello-server-jar   # gives gradle + JDK 25
nix develop nixpkgs#alejandra             # alejandra + its deps
```

## Cargo Unit Test Runtime Inputs

Use `packageTestInputs.<cargo-package> = [ pkgs.tool ];` when a package's tests
spawn a runtime command, and `packageTestEnv.<cargo-package>` for package-local
test environment variables. These apply to aggregate test binaries, per-test
case derivations, and coverage runs.

## Cargo Unit Benchmarks

[`ix.cargoUnit.buildWorkspace`](lib/rust/cargo-unit.nix) exposes Cargo `[[bench]]`
roots under `benchmarks` when a target set includes `--benches` or a specific
`--bench <name>`. Tango benches work as Nix artifacts, including the usual
Linux/macOS `cargo:rustc-link-arg-benches=-rdynamic` build-script line.

```nix
let
  previous = ix.cargoUnit.buildWorkspace {
    src = previousSrc;
    workspaceRoot = previousSrc;
    cargoArgs = [ "--workspace" "--benches" ];
  };
  next = ix.cargoUnit.buildWorkspace {
    src = nextSrc;
    workspaceRoot = nextSrc;
    cargoArgs = [ "--workspace" "--benches" ];
  };
in
next.compareTangoBenchmarks {
  baseline = previous;
  args = [
    "--time"
    "1"
    "--fail-threshold"
    "5"
    "--fail-fast"
  ];
}
```

The comparison runs each matching candidate benchmark binary with `compare`
against the previous workspace's `benchmarkPlan`. It fails on Tango's significant
regression signal and writes one log per benchmark under `$out/logs`; tune the
threshold with Tango's `--fail-threshold` flag.

## Cargo Unit Coverage

`ix.cargoUnit.buildWorkspace` exposes `coverageReport` when the target set
contains tests. Build the workspace with `extraRustcArgs` passing
`-Cinstrument-coverage`; the report derivation runs each test binary, merges the
LLVM profiles, and writes normalized LCOV to `$out/lcov.info`.
Coverage tests run from writable package-source copies by default, matching
Cargo's expectation that tests can create package-local runtime files.
The selected Rust toolchain must include matching LLVM tools, or
`makeCoverageReport` must receive explicit `llvmCov` and `llvmProfdata` paths.

```nix
ix.cargoUnit.buildWorkspace {
  src = workspaceSrc;
  workspaceRoot = workspaceSrc;
  cargoArgs = [
    "--workspace"
    "--tests"
  ];
  profile = "dev";
  extraRustcArgs = [ "-Cinstrument-coverage" ];
}
```

Use `makeCoverageReport { testArgsByPackage = { my-crate = [ "--skip" "slow" ]; }; }`
when a package needs the same libtest arguments every coverage run.

## Cargo Unit Sanitizers

Use native sanitizers when Rust changes touch `unsafe`, FFI, allocator-sensitive
code, async runtime integration, networking, or concurrency that Miri cannot
execute. Keep Miri in the validation ladder for undefined behavior it can model;
native sanitizers cover compiled integration paths.

The smallest native sanitizer smoke is an AddressSanitizer test pass on Linux:

```sh
CARGO_TARGET_DIR=target/sanitizers/address \
RUSTFLAGS="-Zsanitizer=address" \
RUSTDOCFLAGS="-Zsanitizer=address" \
cargo test --workspace --tests -Zbuild-std --target x86_64-unknown-linux-gnu
```

For a GitHub Actions smoke job, run the same command after checkout on an
Ubuntu runner with rustup enabled. `rust-toolchain.toml` pins the nightly
toolchain and includes `rust-src`, which `-Zbuild-std` needs.

| Mode | Run when | Supported target triples |
| --- | --- | --- |
| AddressSanitizer (`address`) | First native sanitizer for ix Rust packages. It catches bounds errors, use-after-free, invalid free, double-free, and many stack lifetime bugs. Linux ASan also enables leak detection by default. | `aarch64-apple-darwin`, `aarch64-unknown-fuchsia`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-unknown-fuchsia`, `x86_64-unknown-freebsd`, `x86_64-unknown-linux-gnu` |
| ThreadSanitizer (`thread`) | Concurrency changes where races are plausible. It needs visibility into synchronization, so partially instrumented code can report false positives, and Rust's upstream docs call out unsupported atomic fences. | `aarch64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-unknown-freebsd`, `x86_64-unknown-linux-gnu` |
| MemorySanitizer (`memory`) | Uninitialized-read hunts where every dependency in the process can be rebuilt with MSan instrumentation. Treat false positives as expected when C/C++ or Rust std stays uninstrumented. | `aarch64-unknown-linux-gnu`, `x86_64-unknown-freebsd`, `x86_64-unknown-linux-gnu` |
| LeakSanitizer (`leak`) | Leak budgets for long-running services or focused tests where leaks are the signal. Prefer ASan's Linux leak coverage unless a separate leak-only run gives a cleaner failure. | `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` |

The other Rust sanitizer modes are specialized. Reach for CFI, DataFlow,
HWAddress, MemTag, SafeStack, ShadowCallStack, or RealtimeSanitizer only when
the package owns that hardening, hardware, ABI, kernel, or realtime constraint.
Rust's upstream target matrix and flag details live in the
[Unstable Book sanitizer chapter](https://doc.rust-lang.org/unstable-book/compiler-flags/sanitizer.html).

## Rust formatting

The repo does not enforce `rustfmt`. Running `cargo fmt` will reflow lines
unrelated to your change (attribute macros, `assert_eq!` call wrapping, etc.)
and produce noise in the diff. Do not run `cargo fmt` before committing.

Rust style is enforced through Clippy (the repo's `llm-clippy` fork) and code
review. See [Rust style](AGENTS.md#rust-style) for the house conventions.

## Running the fork Clippy locally

CI uses a custom `clippy-driver` from the
[`indexable-inc/clippy`](https://github.com/indexable-inc/clippy) fork that adds
two workspace-level lints (`fallible_int_fallback`, `anonymous_tuple_return_type`)
denied across the workspace. A plain `cargo clippy` will not catch these.

To run the same check CI runs, build the per-unit clippy derivation for the
crate you are editing:

```sh
nix build .#ciChecks.x86_64-linux.rust-<crate-name>.clippy
```

Replace `<crate-name>` with the kebab-case package name, e.g.:

```sh
nix build .#ciChecks.x86_64-linux.rust-git-log-pretty.clippy
```

To list every available clippy derivation:

```sh
nix eval --json .#ciChecks.x86_64-linux --apply 'cs: builtins.attrNames cs' | jq '.[] | select(test("clippy"))'
```

## Coding standards

The full style guide lives in [AGENTS.md](AGENTS.md). Skim the section that matches what you're touching:

- [Writing style](AGENTS.md#writing-style) — prose in docs, READMEs, comments, issues, PR descriptions.
- [Inline comments](AGENTS.md#inline-comments) — when a comment earns its place and when to delete it.
- [Rust style](AGENTS.md#rust-style) — naming, module layout, type annotations.
- [Python style](AGENTS.md#python-style) — uv project shape, `zuban --strict` + `ruff` (ANN explicit annotations, TID251 no `typing.cast`) defaults.
- [Nix style (astlog enforced)](AGENTS.md#nix-style-astlog-enforced) — the hard rules `nix run .#lint` checks.
- [Module conventions](AGENTS.md#module-conventions) and [Image conventions](AGENTS.md#image-conventions) — option shape, registry placement, no `..` paths.
- [Dependency intake](AGENTS.md#dependency-intake) — how new external artifacts enter the repo (lockfiles, generated catalogs, fetcher choice).

The lint app enforces the mechanical Nix rules. Reviewers enforce the prose and architecture rules.

## READMEs

A README is the front page a stranger judges the project by, so it is held to a higher bar than other prose: minimal, easy to understand, and it shows the thing working. The shape, top to bottom:

1. **One-line pitch** — what it is and why it exists, in the first sentence.
2. **What it does** — a short paragraph or two of the mental model. No feature laundry list.
3. **Quickstart** — the smallest real, runnable example: the exact commands or code a reader copies to see the tool do its job, and one line saying what they should see. A README without a working example is a brochure.
4. **Pointers** — where the deeper docs live (`doc/<package>/overview.md`), nothing duplicated from them.

Prefer deleting a section to padding it. The repo root [README.md](README.md) is the exemplar of the shape at monorepo scale; [packages/sqlmerge](packages/sqlmerge/README.md) at single-package scale.


## Commit messages

One logical change per commit; see the [Workflow](AGENTS.md#workflow) section for the full convention. The summary:

- Subjects are imperative, lowercased, no trailing period, with an optional `scope:` prefix (`platform:`, `minecraft:`, etc.).
- Bodies explain the *why* the diff cannot show, not the *what*. Skip the body when the subject says everything.
- Avoid `fix stuff`, `WIP`, `address review feedback` (name the feedback), and mixed-concern subjects.
- Use `git commit -m "..." -- <paths>` to commit a specific set of files atomically.
- If the commit fixes a tracked issue, include `Fixes #123` / `Closes #123` in the body.

## Pull requests

Inside the monorepo, a pull request is the optional review path and targets
`main`; verified changes may also push directly. The description should answer
the same "why" question the commit body answers, plus anything reviewer-only:
rollout plan, known follow-ups, and reviewer-specific context.

None of that applies to `indexable-inc/index`, where pull requests are closed
on sight. See "How to get a change made" at the top of this file.
