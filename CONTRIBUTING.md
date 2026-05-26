# Contributing

Bug reports and enhancement requests go to [GitHub Issues](https://github.com/indexable-inc/index/issues). Security reports follow [SECURITY.md](SECURITY.md) instead. Code changes land through pull requests against the `main` branch.

## Local setup

Run the repo lint before pushing:

```sh
nix run .#lint
```

It checks Nix formatting (nixfmt), Statix, Deadnix, and the repo's ast-grep rules. CI runs the same derivation as a flake check.

The repo ships a tracked git pre-commit hook at `.githooks/pre-commit` that calls the lint app. To activate it locally, `direnv allow` in the repo root: `.envrc` exports `core.hooksPath` so git uses the tracked hook. No additional shell or framework is needed.

There is no `devShells.default` to enter for routine work. Reach for the per-package shell when you need build dependencies for a specific artifact, e.g.

```sh
nix develop .#minestom-hello-server-jar   # gives gradle + JDK 25
nix develop nixpkgs#nixfmt                # nixfmt + its deps
```

## Cargo Unit Benchmarks

[`ix.cargoUnit.buildWorkspace`](lib/cargo-unit.nix) exposes Cargo `[[bench]]`
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

## Coding standards

The full style guide lives in [AGENTS.md](AGENTS.md). Skim the section that matches what you're touching:

- [Writing style](AGENTS.md#writing-style) — prose in docs, READMEs, comments, issues, PR descriptions.
- [Inline comments](AGENTS.md#inline-comments) — when a comment earns its place and when to delete it.
- [Rust style](AGENTS.md#rust-style) — naming, module layout, type annotations.
- [Python style](AGENTS.md#python-style) — uv project shape, basedpyright defaults.
- [Nix style (ast-grep enforced)](AGENTS.md#nix-style-ast-grep-enforced) — the hard rules `nix run .#lint` checks.
- [Module conventions](AGENTS.md#module-conventions) and [Image conventions](AGENTS.md#image-conventions) — option shape, registry placement, no `..` paths.
- [Dependency intake](AGENTS.md#dependency-intake) — how new external artifacts enter the repo (lockfiles, generated catalogs, fetcher choice).

The lint app enforces the mechanical Nix rules. Reviewers enforce the prose and architecture rules.

## Commit messages

One logical change per commit; see the [Workflow](AGENTS.md#workflow) section for the full convention. The summary:

- Subjects are imperative, lowercased, no trailing period, with an optional `scope:` prefix (`platform:`, `minecraft:`, etc.).
- Bodies explain the *why* the diff cannot show, not the *what*. Skip the body when the subject says everything.
- Avoid `fix stuff`, `WIP`, `address review feedback` (name the feedback), and mixed-concern subjects.
- Use `git commit -m "..." -- <paths>` to commit a specific set of files atomically.
- If the commit fixes a tracked issue, include `Fixes #123` / `Closes #123` in the body.

## Pull requests

PRs target `main` and need passing required status checks, currently `flake-check` and `chatgpt-codex-connector reviewed head`. The PR description should answer the same "why" question the commit body answers, plus anything reviewer-only (rollout plan, known follow-ups).
