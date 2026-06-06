# Symphony Elixir

This directory contains the Elixir/OTP runtime that lowers `.sym`
workflows to an IR run graph and walks it. See `../README.md` for the
project overview, file layout, env vars, and API.

## Environment

- Elixir: `1.19.x` (OTP 28), pinned in `mise.toml`.
- Install deps: `mix deps.get`.
- Main quality gate: `make all` (which runs `make setup`, `make build`,
  `make fmt-check`, `make lint`).

## Codebase-Specific Conventions

- Runtime config is loaded from the process environment at boot via
  `SymphonyElixir.Config`. Prefer adding new knobs there rather than
  reading `System.get_env/1` ad hoc.
- Workflows (`workflows/*.sym`) are hot-reloaded by
  `SymphonyElixir.WorkflowCatalog` and skills (`skills/*.md`) by
  `SymphonyElixir.Catalog`, both on a 1s tick; no restart needed for
  content changes.
- Workspace safety is critical:
  - Never run a Codex turn with cwd inside the source repo. Every run
    gets a fresh `git worktree add` under `SYMPHONY_WORKSPACES_DIR`.
  - `SymphonyElixir.PathSafety.canonicalize/1` is the gate; any new
    code that resolves a workspace-relative path should route through
    it.
- Runtime behavior is stateful and concurrency-sensitive: preserve
  retry, resume-on-boot, and workspace-cleanup semantics in
  `SymphonyElixir.Runtime` and `SymphonyElixir.IR.Store`.

## Tests and Validation

Run targeted tests while iterating, then run full gates before
handoff:

```bash
make all
mix test
```

## Required Rules

- Public functions (`def`) in `lib/` should have an adjacent `@spec`.
- `defp` specs are optional.
- `@impl` callback implementations are exempt from the `@spec` rule.
- Keep changes narrowly scoped; avoid unrelated refactors in the same
  PR.
- Follow existing module/style patterns in `lib/symphony_elixir/*`.

## PR Requirements

- PR body must follow `../.github/pull_request_template.md`.
- Validate PR body locally when needed:

```bash
mix pr_body.check --file /path/to/pr_body.md
```

## Docs Update Policy

If behavior/config changes, update docs in the same PR:

- `../README.md` for the project concept, file layout, env vars, API.
- `../docs/setup.md` for host setup / runtime credentials.
