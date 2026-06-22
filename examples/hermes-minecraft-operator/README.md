# Hermes Minecraft Operator

A [Hermes agent](../hermes-agent/) operating a Paper Minecraft server. Two nodes in one east-west group:

```
players ──ipv4──> minecraft (Paper 26.1.2 + RCON)
                      ^
                      | RCON (east-west only)
                  hermes (agent + MCP `run_command` tool)
```

The agent gets exactly one game-facing capability: a typed `run_command(command) -> response` MCP tool that speaks RCON to the server console ([`mcp/rcon_mcp.py`](mcp/rcon_mcp.py)). "Whitelist my friend", "shrink the world border to 2000", or a daily player-count report become chat requests instead of console sessions, and the tool's schema is the whole attack surface: one console-command string, parsed by the Minecraft server's own grammar and permission model — no argv, no shell, no file access on the game node.

## Shape

- [`ix.nix`](ix.nix) — the two-node fleet. Players reach the game over public ipv4; RCON is only routable inside the `hermes-minecraft` group.
- [`minecraft.nix`](minecraft.nix) — Paper with `rcon.enable = true` and whitelist enforcement from first boot.
- [`operator.nix`](operator.nix) — layers the MCP server and an operator persona on the shared [`../hermes-agent/hermes.nix`](../hermes-agent/hermes.nix) composition. The RCON host/port are read off the minecraft node's evaluated config, so they cannot drift.
- [`rcon.nix`](rcon.nix) — the shared RCON credential. Committed plaintext like the survival example's forwarding secret: east-west-scoped, obviously a change-me.
- [`documents/SOUL.md`](documents/SOUL.md) — the persona: read-before-write, quote real server responses, destructive commands only on explicit request.

## Run

```sh
# From the index repo root.
nix run .#hermes-minecraft-operator-up
```

Seed the model key and open a chat:

```sh
printf 'OPENROUTER_API_KEY=%s\n' "$OPENROUTER_API_KEY" \
  | ix shell hermes -- sudo install -m0400 -o hermes -g hermes /dev/stdin /run/secrets/hermes.env
ix shell hermes -- sudo systemctl restart hermes-agent
ix shell hermes -- hermes chat
```

(Want this over Telegram instead of a terminal? Add `telegram = true` to the `_module.args.hermes` bag in [`operator.nix`](operator.nix) and follow the [hermes-telegram README](../hermes-telegram/README.md).)

## Things to ask it

Whitelist management (the server enforces the whitelist from first boot):

> **you:** add Steve and Alex to the whitelist
>
> **hermes:** Done. The server said `Added Steve to the whitelist` and `Added Alex to the whitelist`. `whitelist list` now shows 2 players.

World-border tweaks:

> **you:** shrink the border to 2000 blocks over 10 minutes
>
> **hermes:** Current border is 60000 wide. Running `worldborder set 2000 600` — server replied `Setting the border to 2000.0 blocks wide over 600 seconds`. I announced it in-game first with `say`.

The daily player-count report:

> **you:** every evening tell me how many players were on
>
> **hermes:** Scheduled: a cron job at 19:00 runs `list` and posts the result here. You can check it with `hermes cron` from a shell, or just ask me.

The cron job lives in Hermes' own scheduler (`$HERMES_HOME/cron/jobs.json`), created conversationally — nothing to deploy.

## The credential, honestly

RCON is password-authenticated and the password is committed in [`rcon.nix`](rcon.nix). That is acceptable here because the RCON port is only reachable inside the fleet's east-west group (the public internet sees the game port, not the console), and it keeps the generated up wrapper working with zero manual steps. To rotate: edit `rcon.nix`, `ix fleet switch`, then delete `/var/lib/minecraft/.ix-rcon-password` on the minecraft node and restart it (the seed only writes when the file is absent).

## Bad fit if

- You want the agent to edit server files, install plugins, or restart the unit. Those are declarative fleet concerns; this preset deliberately gives the agent a console, not a shell, on the game node.
- You want multiple agents or per-player personas. The agent state is single-tenant; run another hermes node.
