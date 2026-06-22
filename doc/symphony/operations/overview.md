# Operations: running Symphony

Genre: recipe. How to launch the runtime, the env vars it reads, the dashboard and
API it serves, the triggers that fire workflows, and the NixOS module that deploys
it. Source of truth for env vars is `config.ex`; for the unit, `default.nix`,
`bin/run-nix`, and `../../modules/services/symphony/default.nix`.

## Launch

```sh
export SYMPHONY_PRIMARY_REPO=/path/to/your/repo
nix run .#symphony
# dashboard on http://127.0.0.1:4040
```

`symphony` is the single flake output, a Nushell launcher wrapping `bin/run-nix`
(`default.nix:148-171`). It refuses to start without an authenticated `codex` on
PATH (`bin/run-nix:23-26`); `codex` is intentionally not in the package's
`runtimeInputs`, so the binary and its credentials stay host-owned
(`default.nix:154-156`). `runtimeInputs` provide `bash`, `cacert`, `coreutils`,
`elixir`, `erlang`, `gh`, `git`, `openssh` (`default.nix:157-166`).

`bin/run-nix` (`bin/run-nix:1-63`): stages this source tree into
`$SYMPHONY_STATE_DIR/runtime` (so `mix` can write `_build` without touching a
read-only nix-store or live working-tree checkout), sets `SYMPHONY_ROOT` to that
copy, creates the state dirs, then runs `mix deps.get`, `mix compile
--warnings-as-errors`, and `exec mix run --no-halt` (which boots
`SymphonyElixir.Application`). `MIX_ENV` defaults to `prod`.

Run an external pack:

```sh
export SYMPHONY_PACK_DIR=/path/to/your/pack
export SYMPHONY_PRIMARY_REPO=/path/to/your/primary/repo
nix run github:indexable-inc/index#symphony
```

Develop:

```sh
nix develop .#symphony      # Elixir 1.19 / OTP 28, plus codex, gh, git
cd packages/agent/symphony/elixir
make all                    # setup, compile -Werror, fmt-check, credo
mix test
```

## State directories

`bin/run-nix` anchors mutable state under `SYMPHONY_STATE_DIR` (default
`$HOME/.local/state/symphony`), separate from the staged runtime copy that is
wiped on every restart (`bin/run-nix:6-21`):

| var | default | holds |
| --- | --- | --- |
| `SYMPHONY_STATE_DIR` | `$HOME/.local/state/symphony` | root of mutable state |
| `SYMPHONY_RUNS_DIR` | `$SYMPHONY_STATE_DIR/runs` | per-run `RunGraph` JSON (`runs/ir/*.json`) and `cron_state.json` |
| `SYMPHONY_WORKSPACES_DIR` | `$SYMPHONY_STATE_DIR/workspaces` | per-run git worktrees (accepts the legacy `SYMPHONY_WORKSPACE_ROOT`) |
| `SYMPHONY_LOGS_ROOT` | `$SYMPHONY_STATE_DIR/log` | run logs |
| `SYMPHONY_HTTP_PORT` | `4040` | Phoenix listener (accepts `SYMPHONY_PORT`) |

`Config` reads its env snapshot once at boot; to pick up an env change, restart the
BEAM. Pack files and skills hot-reload without a restart (`config.ex:1-9`).

## Core configuration (`config.ex`)

Required: `SYMPHONY_ROOT` (set by the launcher) and `SYMPHONY_PRIMARY_REPO`
(`config.ex:11-14`). Pack selection and runtime paths are covered in
[pack](../pack/overview.md). Other commonly-set knobs (`config.ex:26-140`):

- `SYMPHONY_CODEX_COMMAND` (default `codex app-server`), `SYMPHONY_CLAUDE_COMMAND`
  (default `claude`), `ANTHROPIC_API_KEY` (required for any Claude-model node).
- `SYMPHONY_ROOM_SERVER_URL` for `:local`/`{:room, url}` placements;
  `SYMPHONY_ROOM_REGISTRY_URL`/`_TOKEN`/`SYMPHONY_ROOM_ADVERTISE_HOST` for the
  central room.ix.dev that aggregates run transcripts.
- `SYMPHONY_SUBRUN_MAX_DEPTH` (default 8), `SYMPHONY_CATALOG_POLL_MS` (1000),
  `SYMPHONY_CRON_POLL_MS` (60000), `SYMPHONY_SLACK_POLL_MS` (60000).
- GitHub App (commit/push as a bot): `SYMPHONY_GITHUB_APP_ID`,
  `SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64` (base64 so it fits one env line),
  `SYMPHONY_GITHUB_APP_OWNER_REPO`, `SYMPHONY_BOT_USERNAME`, `SYMPHONY_BOT_EMAIL`.
- Integrations: `LINEAR_API_KEY`, `GITHUB_TOKEN`, the webhook secrets, and the
  Slack tokens (see [Triggers](#triggers)). `.env.example` is the full list.

## Boot supervision tree (`application.ex`)

`SYMPHONY_ROLE` selects the tree (`application.ex:45-83`). The default
`control_plane` boots, in order: `Phoenix.PubSub`, `Task.Supervisor`, `Config`,
`GithubApp`, `Catalog` (skills), `WorkflowCatalog` (workflows), `CronState`,
`Runtime.Registry`, `Runtime.Placement`, `Runtime.RuntimeRegistry`,
`Runtime.Supervisor`, `Triggers.Slack`, `Triggers.Cron`, and the Phoenix
`Endpoint`. After the tree starts it calls `Runtime.Supervisor.resume_pending/1`
to reload and resume non-terminal runs. The `worker` role boots only enough to
dial the control plane and provision per-run room-servers (no DB, triggers,
engine, or HTTP).

## Placements

Each agent node picks where its engine process runs with `location:` in the
`.sym` ([envelope](../engine/contract.md#envelope)). `host` runs codex directly on
the Symphony machine as `SYMPHONY_HOST_USER` (no VM); `ixvm` runs it in a
short-lived iXVM; `local`/`room` use `SYMPHONY_ROOM_SERVER_URL`
(`packages/agent/symphony/docs/setup.md:53-73`). When an `ixvm` placement fails to
provision, the run retries on `SYMPHONY_PLACEMENT_FALLBACK` (default `host`; also
`remote`, `local`, `none`, `config.ex:74-83`). Host placement needs
`SYMPHONY_HOST_USER` (and optionally `_GROUP`, `_WORKSPACES_DIR`, `_KEEP`); on
NixOS, `services.symphony.hostRuntime` wires the polkit grant and PATH it requires.
The lifecycle is owned by [`Runtime.Placement`](../engine/overview.md#placement).

## Dashboard and JSON API

The Phoenix `Endpoint` serves on `SYMPHONY_HTTP_PORT` (default 4040) and is
described as the optional observability UI and API (`endpoint.ex:1-4`). Routes
(`*_web/router.ex`):

LiveView dashboard (`router.ex:25-40`):

- `/` and `/ir` - the IR runs index; also carries the schema-driven control to
  start a workflow by name. `/ir/:run_id` - one run in detail with per-node state
  pills. Live updates ride `Runtime.Events` PubSub, so pills move without polling
  (`live/ir_runs_live.ex:13-18`).
- `/workflows`, `/workflows/:name` - the workflow catalog (and parse errors).
- `/skills`, `/skills/:name` - the skill catalog.
- `/statistics` - GitHub-backed stats for bot-authored PRs
  (`SYMPHONY_GITHUB_STATS_QUERY`).

JSON API under `/api/v1` (`router.ex:42-61`):

- `POST /runs` - manual-trigger enqueue: `{"workflow": "..", "input": {..}}`
  starts that `.sym`; without `workflow` it fires every `on manual` workflow
  (`controllers/api_controller.ex:1-29`).
- `GET /ir/schema` - the runtime enum vocabulary (drives form option lists).
- `GET /ir/runs`, `POST /ir/runs`, `GET /ir/runs/:run_id` - list/create/read runs.
- `POST /ir/runs/:run_id/{cancel,rerun,clear-failed}` and
  `.../nodes/:node_id/retry` - the operator hooks; a run with no live process
  returns 409 (`controllers/ir_run_controller.ex:96-110`).
- `POST /triggers/{linear,github,slack/events}` - the webhook receivers.

## Triggers

Producers all funnel through `Runtime.Ingress` and the shared `Runtime.Trigger`
matcher (see [engine](../engine/overview.md#triggers-and-ingress)). The trigger
kinds and their `.sym` `on` syntax are in [dsl](../dsl/overview.md#triggers-the-on-header).

- **Cron** (`triggers/cron.ex`) - a GenServer ticking every
  `SYMPHONY_CRON_POLL_MS` (default 60s). It persists `last_fired_at` per workflow
  via `CronState`; a brand-new cron workflow is seeded without firing (no boot
  catch-up), and at most one catch-up fire happens per restart (the
  `systemd Persistent=true` semantic, `triggers/cron.ex:8-22`). Schedules are
  parsed by `CronExpression` (`cron_expression.ex:1-30`): standard 5-field cron
  with `*`, lists, ranges, and `*/n` steps, plus `@yearly`/`@monthly`/`@weekly`/
  `@daily`/`@hourly` nicknames, all in UTC.
- **Webhooks** - `LinearWebhookController`, `GithubWebhookController`, and
  `SlackEventsController` verify the inbound signature against
  `LINEAR_WEBHOOK_SECRET`/`GITHUB_WEBHOOK_SECRET`/`SLACK_SIGNING_SECRET` (absent
  rejects 401), extract the event, dedup, and call `Ingress.start_by_trigger/2`.
  `RawBodyReader` retains the raw body for signature checks (`*_web/raw_body_reader.ex`).
- **Slack huddle** (`triggers/slack.ex`) - an opt-in poller enabled by
  `SLACK_BOT_OAUTH_TOKEN`.
- Failed (and optionally successful) cron runs post to Slack per
  `SYMPHONY_SLACK_NOTIFY_CHANNEL`, `SYMPHONY_SLACK_NOTIFY_CRON_FAILURES` (default
  true), and `SYMPHONY_SLACK_NOTIFY_CRON_WORKFLOWS` (`config.ex:107-110`).

## NixOS module (`modules/services/symphony`)

`services.symphony` is a minimal opinionated systemd unit
(`modules/services/symphony/default.nix:1-7`). Key options: `package` (this flake's
`symphony`), `user` (default `symphony`), `stateDir` (`/var/lib/symphony`),
`httpPort` (4040), `primaryRepo`, `repoRoot`, `workflowPack`/`packDir`, the room
options, `extraEnvironment` (non-secret config), and `environmentFile` or
`secretsCommand` for secrets (`modules/services/symphony/default.nix:26-161`).
`secretsCommand` wraps `ExecStart` (designed for Bitwarden `bws run -- ...`). The
module creates `stateDir` and the `workspaces`/`runs`/`log` subdirs via tmpfiles,
sets the `SYMPHONY_*` env, and keeps sandboxing permissive because the service
spawns codex subprocesses and clones repos
(`modules/services/symphony/default.nix:258-347`). `hostRuntime.enable` adds the
polkit rule scoping `org.freedesktop.systemd1.manage-units` to the
`symphony-host-` unit prefix so the non-root service can run codex as another user
(`modules/services/symphony/default.nix:225-244`); it requires `user` and
`roomServerPackage`.

```nix
services.symphony = {
  enable = true;
  package = index.packages.${system}.symphony;
  packDir = "/var/lib/symphony-pack";
  primaryRepo = "/var/lib/repos/my-app";
  environmentFile = "/run/secrets/symphony.env";
};
```

## Quality gates

The required lane (compile `--warnings-as-errors`, `mix format --check-formatted`,
`mix credo`, `mix test`) runs sandboxed in CI as the `symphony-elixir` flake check,
defined in `default.nix` (`elixirCheck`, exposed via `passthru.tests.elixir`,
`default.nix:74-176`). After changing `elixir/mix.lock`, refresh the `fetchMixDeps`
hash there (`default.nix:40-54`). The advisory lane (`make quality`: format check,
Credo, Sobelow, `deps.audit`, Dialyzer, plus `mix coveralls`) stays a local run,
since those tools want network or large mutable caches; see
`packages/agent/symphony/docs/quality.md`.
