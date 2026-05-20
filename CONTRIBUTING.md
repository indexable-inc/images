# Contributing

Bug reports and enhancement requests go to [GitHub Issues](https://github.com/indexable-inc/index/issues). Security reports follow [SECURITY.md](SECURITY.md) instead. Code changes land through pull requests against the `development` branch.

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

PRs target `development` and need one approving review plus passing status checks (`flake-check` and the `Analyze (*)` CodeQL jobs). The PR description should answer the same "why" question the commit body answers, plus anything reviewer-only (rollout plan, known follow-ups).
