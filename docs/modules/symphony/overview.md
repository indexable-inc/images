# symphony

`modules/services/symphony/default.nix` is a minimal, opinionated systemd unit
for the Symphony runtime (`packages/symphony`, the Phoenix/Elixir agent
orchestrator whose `package` provides `/bin/symphony`). The module is
deliberately small: it reads secrets from an `EnvironmentFile` you control, so
any secret manager (sops-nix, agenix, Bitwarden Secrets Manager, ...) wires
underneath.

Option namespace: `services.symphony` (`default.nix:26`).

## Public surface (options)

- `enable` (`default.nix:27`).
- `package` (package, required) - the Symphony build providing `/bin/symphony`
  (`default.nix:29`).
- `user` (str, default `symphony`) - run-as user (`default.nix:34`).
- `stateDir` (path, default `/var/lib/symphony`) - runs, workspaces, logs,
  staged runtime (`default.nix:40`).
- `httpPort` (port, default 4040) - Phoenix HTTP listener (`default.nix:46`).
- `primaryRepo` / `repoRoot` (nullable path) - `SYMPHONY_PRIMARY_REPO` /
  `SYMPHONY_REPO_ROOT` (`default.nix:52`, `:58`).
- `workflowPack` (str, default `example`) / `packDir` (nullable path,
  precedence) - built-in vs external workflow pack (`default.nix:64`, `:70`).
- `roomRegistryUrl` / `roomAdvertiseHost` / `roomServerUrl` (nullable str) -
  room.ix.dev registration and per-run room-server addressing
  (`default.nix:76`, `:88`, `:100`).
- `extraEnvironment` (attrs of str) - non-secret env (LINEAR_WORKSPACE_SLUG,
  SYMPHONY_BOT_*, ...) (`default.nix:109`).
- `environmentFile` (nullable path) - systemd EnvironmentFile holding secrets
  (LINEAR_API_KEY, GITHUB_TOKEN, webhook secrets, Slack tokens, ...)
  (`default.nix:120`).
- `secretsCommand` (nullable list of str) - a wrapper that injects secrets and
  execs its trailing args, e.g. `bws run -- ...`; prepended to ExecStart
  (`default.nix:134`).
- `path` (list of package) - extra packages on the service PATH, e.g.
  `pkgs.bws` (`default.nix:157`).
- `hostRuntime` (submodule) - the host codex placement (`default.nix:163`):
  `enable`, `user`, `group`, `workspacesDir`, `roomServerPackage`, `keep`. When
  enabled, a workflow node declaring `location: host` runs codex directly on this
  machine as a real OS user via transient `systemd-run --uid` units.

## Key internals

- **Assertions** (`default.nix:214-223`): `hostRuntime.user` and
  `hostRuntime.roomServerPackage` must be set when `hostRuntime.enable`.
- **polkit grant** (`default.nix:231-244`): host runtime calls
  `StartTransientUnit` over D-Bus to run codex as another user; a non-root
  service needs polkit authorization, scoped to the `symphony-host-` unit-name
  prefix so it cannot manage unrelated units.
- **Environment mapping** (`default.nix:279-320`): typed options become
  `SYMPHONY_*` env vars (state/workspaces/runs/logs dirs, HTTP port, workflow
  pack, room URLs, host-runtime vars), merged with `extraEnvironment`.
- **ExecStart** (`default.nix:326-333`): `<secretsCommand?> <package>/bin/symphony`.
- **tmpfiles** create `stateDir` and `workspaces`/`runs`/`log` subdirs at 0750
  (`default.nix:258-263`).

## What it produces

`systemd.services.symphony` (`default.nix:265`), wanted by `multi-user.target`,
after `network-online.target`. Sandboxing is intentionally light
(`default.nix:339-347`: only `NoNewPrivileges`, `PrivateTmp`,
`ProtectKernelTunables`/`Modules`/`ControlGroups`) because Symphony spawns codex
subprocesses and clones git repos. No port claim or health check is declared
(the module imports only `config`/`lib`/`pkgs`).

## How it is wired

Auto-discovered as `services/symphony`. The `package` is passed in by the
consumer (this flake's `symphony` default output, built from
`packages/symphony`).
