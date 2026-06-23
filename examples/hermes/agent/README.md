# Hermes Operator VM

One ix node running [Nous Research's Hermes agent](https://hermes-agent.nousresearch.com/) as a long-lived daemon. The upstream NixOS module (`services.hermes-agent.*`) is wired in through [`index.lib.hermesAgent`](../../../lib/default.nix); this preset only chooses the model provider, the persona, and which integrations are turned on.

Defaults: OpenRouter for the model, local SQLite memory, Edge TTS, a filesystem MCP server pointed at the workspace, no messaging platforms. Everything else is opt-in through `_module.args.hermes.*`.

## Run

```sh
# From the index repo root.
nix run .#hermes-agent-up
```

That brings the VM up with the Hermes daemon running but no API key. The next four lines seed the key, restart the unit, and open a chat:

```sh
printf 'OPENROUTER_API_KEY=%s\n' "$OPENROUTER_API_KEY" \
  | ix shell hermes -- sudo install -m0400 -o hermes -g hermes \
      /dev/stdin /run/secrets/hermes.env
ix shell hermes -- sudo systemctl restart hermes-agent
ix shell hermes -- hermes chat
```

The first command writes the env file at the path the systemd unit reads from. The file lives outside `/nix/store` and is readable only by the `hermes` user. To rotate, rewrite it the same way.

## Shape

- [`ix.nix`](ix.nix) wraps the node as a one-node fleet.
- [`hermes.nix`](hermes.nix) is the service composition. It includes the upstream `services.hermes-agent` module and reads a `_module.args.hermes` arg-bag for the integration toggles.
- [`documents/SOUL.md`](documents/SOUL.md) is the agent's persona prompt. It tells the agent it is inside an ix VM, what tooling is on PATH, and which authorities live on the host side.
- [`documents/USER.md`](documents/USER.md) is the long-running user context Hermes injects every session.
- [`secrets.env.example`](secrets.env.example) is the template for `/run/secrets/hermes.env`. Every supported integration's variable name is listed here.

## Enable more providers

Every Tier 1 integration is one flag in the fleet preset plus matching lines in the env file. The flag goes in your override of [`ix.nix`](ix.nix) (or in a separate fleet that imports [`hermes.nix`](hermes.nix)):

```nix
nodes.hermes = {
  modules = [
    index.lib.hermesAgent.nixosModules.default
    ./hermes.nix
  ];
  _module.args.hermes = {
    telegram = true;
    webSearch = "tavily";
    memory = "mem0";
  };
};
```

After rebuild, append the matching credentials to `/run/secrets/hermes.env`:

```sh
ix shell hermes -- sudo tee -a /run/secrets/hermes.env <<'EOF'
TELEGRAM_BOT_TOKEN=...
TELEGRAM_ALLOWED_USERS=123456789
TAVILY_API_KEY=tvly-...
MEM0_API_KEY=...
EOF
ix shell hermes -- sudo systemctl restart hermes-agent
```

### Available toggles

| `_module.args.hermes.*` | Values | Effect |
| --- | --- | --- |
| `telegram` | bool (default `false`) | Long-poll Telegram bot. Outbound only. |
| `discord` | bool (default `false`) | Discord WebSocket gateway. Outbound only. |
| `homeAssistant` | bool (default `false`) | Talk to a Home Assistant instance via `HASS_TOKEN`. |
| `imageGen` | bool (default `false`) | FAL.ai image generation. |
| `webSearch` | `null` (default) / `"tavily"` / `"exa"` / `"firecrawl"` / `"parallel"` | Real web search tool. |
| `tts` | `"edge"` (default, free) / `"elevenlabs"` / `"minimax"` / `"openai"` | Spoken replies. |
| `memory` | `"holographic"` (default, local SQLite) / `"mem0"` / `"supermemory"` / `"honcho"` / `"hindsight"` / `"retaindb"` / `"openviking"` / `"byterover"` | Persistent memory backend. |
| `modelDefault` | model string (default `"anthropic/claude-sonnet-4"`) | Model the agent asks for. |
| `modelBaseUrl` | URL (default OpenRouter) | Point at `https://api.anthropic.com/v1` or `https://api.openai.com/v1` for direct routing. |
| `apiServer` | bool (default `false`) | OpenAI-compatible `hermes api-server`. The one INBOUND toggle: claims a TCP port (`apiServerPort`, default `9119`) and opens the in-guest firewall; reachability is scoped by the node's east-west groups. Set `API_SERVER_KEY` in the env file. |
| `apiServerPort` | port (default `9119`) | Listen port for `apiServer`. |

Per-integration env file overrides (`telegramEnvFile`, `webSearchEnvFile`, etc.) accept a different absolute path when your secret store splits keys across files. They all default to `envFile`, which defaults to `/run/secrets/hermes.env`.

### Sibling presets

Three ready-made shapes build on this composition instead of forking it:

- [`examples/hermes/telegram`](../telegram/) — Telegram chat companion (`telegram = true`, chat-tuned `SOUL.md`, BotFather walkthrough).
- [`examples/hermes/minecraft-operator`](../minecraft-operator/) — the agent operating a Paper Minecraft server through a typed RCON `run_command` MCP tool.
- [`examples/hermes/api-server`](../api-server/) — `apiServer = true` in an east-west group, so LobeChat / Open WebUI / LibreChat on sibling VMs use the agent as their OpenAI endpoint.

## Bad fit if

- You want the agent to install Ubuntu `.deb` packages at runtime. Flip `services.hermes-agent.container.enable = true` upstream-side and accept Docker-in-VM nesting. On ix the agent already has `nix shell nixpkgs#<tool>` against a real NixOS, so the container mode tradeoff usually does not pay off.
- You want one Hermes daemon serving several personas. The state DB (`state.db`, `memories/`, `skills/`) is single-tenant. Spin up a second node with a different `SOUL.md` instead.
- You want inbound webhooks from WhatsApp, Microsoft Teams, Slack, SMS, or email. Those need a public hostname and a port claim. Build a gateway-VM preset rather than adding the surface here.
- You want the OpenAI-compatible `hermes api-server` so LobeChat or Open WebUI can point at this node. That is the [`examples/hermes/api-server`](../api-server/) preset (a port claim on `9119` plus an east-west group), not extra surface on this outbound-only shape.

## What's load-bearing

- The Hermes flake input is pinned to a release tag (`v2026.5.16` at the time of writing) in [`flake.nix`](../../../flake.nix). Bump it with `nix flake update hermes-agent` after the upstream release has aged past the repo's 24-hour intake gate.
- API keys never enter `/nix/store`. The systemd unit reads them through `EnvironmentFile=` from a file the operator drops on the running VM.
- The agent has root inside the VM. Anything that must hold against a misbehaving in-VM process belongs outside the VM (snapshots, registry pushes, source-switch authority). See `CLAUDE.md` for the full trust model.
