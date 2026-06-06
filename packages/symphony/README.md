<p align="center">
  <img src="assets/logo.svg" width="80" alt="Symphony" />
</p>

# symphony

> [!IMPORTANT]
> Symphony is highly experimental software. Use it at your own risk: it can spawn Codex sessions, create branches, open PRs, and mutate Linear/GitHub state when credentials allow it.

Symphony is a boring DAG runtime for deterministic agent workflows. Workflows are written in the `.sym` surface language, lowered to an IR run graph, and walked by a supervised Elixir/OTP runtime with a LiveView dashboard, cron/Slack/Linear/GitHub triggers, and per-run git worktrees. It moved here from the dedicated [indexable-inc/symphony](https://github.com/indexable-inc/symphony) repo (rev `c9e7092`).

Run it from this repo:

```sh
nix run .#symphony
```

The launcher requires an authenticated `codex` on PATH and refuses to start without one. It stages this source tree under `~/.local/state/symphony`, fetches mix deps, and boots the dashboard on http://127.0.0.1:4040. Point `SYMPHONY_PRIMARY_REPO` at a local checkout first; [docs/setup.md](docs/setup.md) and [.env.example](.env.example) cover the full configuration surface.

<img alt="Symphony dashboard" src="https://github.com/user-attachments/assets/eb06f062-3b2d-41a4-a679-94c5c2f847aa" />

## Layout

- [`elixir/`](elixir/): the runtime (DSL parser, IR, runtime supervisor, Phoenix dashboard, triggers).
- [`workflows/example/`](workflows/example/): the bundled pack, intentionally narrow (one manual-trigger `inspect` workflow plus its read-only skill). Real deployments point `SYMPHONY_PACK_DIR` at their own pack.
- [`contracts/fixtures/`](contracts/fixtures/): engine wire fixtures shared with the room-server in the ix monorepo. The Elixir contract tests read them from `../../contracts`, so this directory stays beside `elixir/`.
- [`bin/run-nix`](bin/run-nix): the production entrypoint the `symphony` package wraps.
- [`docs/`](docs/): setup, engine contract, and quality-gate reference.

## Neighbors

- The room stack symphony drives over HTTP (`room-server` and the room UI) lives in the ix monorepo (`crates/room`, `packages/room`).
- `location: ixvm` placements provision VMs from the [`symphony-codex`](../../images/dev/symphony-codex/) image, which carries `room-server` on PATH.
- Deployment goes through the [`symphony` NixOS module](../../modules/services/symphony/) (`services.symphony.*`), with secrets supplied via `environmentFile` or `secretsCommand`.

## Developing

```sh
nix develop .#symphony   # Elixir 1.19 / OTP 28, plus codex, gh, git
cd packages/symphony/elixir
make all                 # setup, compile -Werror, fmt-check, credo
mix test
```

CI runs the same required lane sandboxed as the `symphony-elixir` flake check (see [default.nix](default.nix)); after changing `elixir/mix.lock`, refresh the `fetchMixDeps` hash there. The advisory lane (`make quality`: sobelow, deps.audit, dialyzer, coveralls) stays a local run; see [docs/quality.md](docs/quality.md).
