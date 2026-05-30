## Why `dag-runner` (and not `process-compose` or `devenv-tasks`)

The repo-owned [`dag-runner`](packages/dag-runner/) is the task runner that
powers `nix run .#health-checks` and is the planned replacement for the
sequential per-node loops in [`ix-fleet`](packages/ix-fleet/src/ix_fleet/__init__.py)
(`cmd_up`, `cmd_switch`, `cmd_replace`). It exists despite the
ecosystem-already-provides rule because neither upstream candidate fits the use
case cleanly.

`process-compose` is a long-running supervisor whose default TUI takes over the
alt-screen and clears it on exit, so a fast failure renders as a silent no-op in
scrollback. Inline-progress support was requested upstream and rejected
([F1bonacc1/process-compose#362](https://github.com/F1bonacc1/process-compose/issues/362)).
`dag-runner` uses inline `indicatif` spinners that stay in scrollback after a
run, plus an `--output json` NDJSON event stream, so failures remain visible
and machine-readable.

`devenv-tasks` has the right interactive UX shape, but its task spec is part of
devenv's internal module interface rather than a standalone schema we can pin,
and the binary is not packaged in `nixpkgs` independently of the devenv flake.
Adopting it as an orchestrator would couple us to devenv's release cycle for a
tool that needs to own a small, stable JSON contract.

The switch landed in
[`d9e2fa1`](https://github.com/indexable-inc/index/commit/d9e2fa1) ("lib: add
dag-runner and switch nix run .#health-checks to use it"). Lifecycle scripts
per example (rm-then-up-then-rm) were unchanged; only the orchestrator swapped.
The JSON spec is a top-level `nodes` map of `{ name → { command, depends_on?,
env?, timeout_secs? } }`, owned by this repo and documented in
[`packages/dag-runner/README.md`](packages/dag-runner/README.md).

If a third consumer arrives needing something dag-runner does not, extend the
runner. Pulling in `process-compose` or `devenv-tasks` alongside it for "the
part dag-runner does not do" would defeat the consolidation.
