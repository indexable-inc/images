# Setup

## Local development

```bash
git clone https://github.com/your-org/symphony
cd symphony

export SYMPHONY_PRIMARY_REPO=/path/to/your/repo
nix run .
```

Open http://127.0.0.1:4040 for the dashboard.

The bundled `workflows/example` pack ships a single manual-trigger `.sym`
workflow (`workflows/inspect.sym`) with a read-only `inspect` skill that does
not push anything anywhere. It is intended as a starting point you can copy
into your own pack.

## Running a real workflow pack

Drop your pack directory (a `workflows/` of `.sym` files, a `skills/` of
markdown, and a `repositories.yaml`) anywhere on the host and point Symphony
at it:

```bash
export SYMPHONY_PACK_DIR=/path/to/your/pack
export SYMPHONY_PRIMARY_REPO=/path/to/your/primary/repo
nix run github:your-org/symphony
```

Required runtime credentials depend on which triggers and tools your pack
uses; see `README.md` and `elixir/lib/symphony_elixir/config.ex` for the full
env var list.

Symphony treats the workflow pack as read-only runtime input. Put mutable run
state under `SYMPHONY_RUNS_DIR` and worktrees under `SYMPHONY_WORKSPACES_DIR`;
both default under the runtime state directory when using the Nix wrapper.

Common ones:

- `LINEAR_API_KEY` (Linear graphql tool + webhook enqueue)
- `GITHUB_TOKEN` (dashboard statistics)
- `LINEAR_WEBHOOK_SECRET`, `GITHUB_WEBHOOK_SECRET`, `SLACK_SIGNING_SECRET`
  (webhook receivers)
- `SYMPHONY_GITHUB_APP_ID`, `SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64`,
  `SYMPHONY_GITHUB_APP_OWNER_REPO` (commit/push as a bot identity)
- `SYMPHONY_BOT_USERNAME`, `SYMPHONY_BOT_EMAIL` (git author when the App is
  configured)

Codex must already be installed and authenticated on the host.

## Choosing a placement

Each agent node picks where its codex session runs with a `location:` field in
the `.sym` workflow:

```
implement <- agent {
  engine: codex
  model: "gpt-5.3-codex"
  permissions: workspace_write
  location: host   # or: ixvm, room, local
  prompt: skill "implement"
}
```

`host` runs codex directly on the Symphony machine as a real OS user
(`SYMPHONY_HOST_USER`) inside that user's home directory, with no VM, so the
agent can read and write that user's files. `ixvm` runs it inside a
short-lived iXVM. Both stand up a per-run room-server and register it so the
room UI can attach. `local` and `room` use the default
`SYMPHONY_ROOM_SERVER_URL`.

When a node's `ixvm` placement fails to provision before the first turn, the
run retries on the placement named by `SYMPHONY_PLACEMENT_FALLBACK` (defaults
to `host`). On NixOS, set `services.symphony.hostRuntime` to wire the polkit
grant, PATH, and `SYMPHONY_HOST_USER` the host placement needs:

```nix
services.symphony.hostRuntime = {
  enable = true;
  user = "hari";
  roomServerPackage = symphony.packages.${pkgs.system}.room-server;
};
```

## Choosing an engine: Codex or Claude

An agent node names its engine directly in the `.sym` workflow with the
`engine:` field (`codex` or `claude`); the room-server's engine host runs the
turn through the matching adapter.

```
report <- agent {
  engine: claude
  model: haiku
  permissions: read_only
  prompt: inline "write a status report"
}
```

A Claude model means `claude-*` or the `opus` / `sonnet` / `haiku` aliases.
Claude turns are billed against `ANTHROPIC_API_KEY`. The codex-only `sandbox`
/ `approval_policy` skill fields do not apply to Claude turns.

## Production deployment (NixOS)

```nix
{
  inputs.symphony.url = "github:your-org/symphony";

  outputs = { self, nixpkgs, symphony, ... }: {
    nixosConfigurations.host = nixpkgs.lib.nixosSystem {
      modules = [
        symphony.nixosModules.symphony
        ({ pkgs, ... }: {
          services.symphony = {
            enable = true;
            package = symphony.packages.${pkgs.system}.default;
            packDir = "/var/lib/symphony-pack";
            primaryRepo = "/var/lib/repos/my-app";
            environmentFile = "/run/secrets/symphony.env";
          };
        })
      ];
    };
  };
}
```

Pair the module with whichever secret store you prefer (sops-nix, agenix,
Bitwarden Secrets Manager via `secretsCommand`, etc). See the module options
in `modules/services/symphony.nix`.
