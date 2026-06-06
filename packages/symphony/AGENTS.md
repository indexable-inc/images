# symphony

An Elixir runtime that orchestrates Codex agent sessions across one or
more git repositories. Workflows are written in the `.sym` surface
language, lowered to an IR run graph the runtime walks; hot-reloaded
`.sym` workflows and markdown skills are the configuration surface. The
room stack (`room-server` and the Tauri/Svelte client) lives in the IX
monorepo; this package is the Elixir runtime that drives it over HTTP.

Repo-wide standards (writing style, Nix style, commit conventions) come
from the index root AGENTS.md. This file holds only the invariants that
are specific to symphony.

Do not commit secrets. Tokens for Linear, GitHub, Slack, Codex, or any
other external system must be supplied through the runtime environment or
host secret manager. The bundled `.env.example` lists the keys the
runtime reads.

## Self-contained operations

Symphony's runtime behavior must not depend on out-of-repo changes to
function. In particular, scheduled work (cron triggers, dispatchers,
auto-healing loops) belongs inside the runtime, driven by Symphony's own
cron scheduler. Do not introduce systemd timers, host nix modules, or any
out-of-repo schedulers as load-bearing pieces of a symphony feature. A
fresh symphony deploy should bring up all of its scheduled work without
needing a paired change in any other repo.

## Workflow packs

The runtime is pack-agnostic. The bundled `workflows/example/` pack is the
public default and is intentionally narrow (a single manual-trigger inspect
skill). Deployers point `SYMPHONY_PACK_DIR` at their own pack to drive real
work. Keep core changes pack-agnostic: no workflow names, repo slugs,
label strings, or ticket schemes hardcoded in `elixir/lib/`.

## Elixir style

The Elixir runtime is the entry point for symphony itself; the room
stack it drives lives in the IX monorepo and is not owned here. Keep
`elixir/lib/` pack-agnostic, with workflow shape carried in `.sym` /
markdown under the active pack directory rather than hardcoded in source.

Prefer Mix tasks and supervised processes over loose scripts. A new
scheduled job is a child of Symphony's cron supervisor, not a host-level
timer.

## Tests

Tests should protect behavior that can regress across boundaries:
module merges, generated units, pack rendering, and runtime contracts
(including the engine wire fixtures in `contracts/fixtures` shared with
the room-server in IX). Avoid asserting facts already obvious from the
literal config under test.

The required lane (compile with warnings as errors, format, credo,
`mix test`) runs sandboxed as the `symphony-elixir` flake check; the
advisory lane is `make quality` in `elixir/`. See `docs/quality.md`.

## Layout

```
default.nix                # symphony launcher package + the elixir check
elixir/                    # Symphony runtime (.sym/IR orchestrator)
workflows/                 # pack-agnostic example pack
contracts/fixtures/        # engine wire fixtures shared with room-server (IX)
docs/                      # package-owned reference
../../modules/services/symphony/  # NixOS module for the runtime
```

Folders should preserve conceptual paths. When siblings share a real
domain, nest them under that domain instead of flattening the name
into repeated dashed prefixes.
