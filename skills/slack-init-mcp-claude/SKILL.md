---
name: slack-init-mcp-claude
description: Set up, verify, and debug Slack's hosted Model Context Protocol server for Claude Code (the Anthropic API / CLI build). Use when the user asks to add/install/setup/init/configure the Slack MCP in Claude Code, wire the Slack bearer token through Vaultwarden or Nushell for Claude Code, or troubleshoot Claude Code Slack MCP problems such as "Failed to connect", "Needs authentication", slack not showing Connected, a missing SLACK_MCP_TOKEN, or mcp__slack__* tools not appearing in a running session. For Codex, use slack-init-mcp instead.
---

# Slack Init MCP for Claude Code

Wire Slack's hosted MCP into Claude Code with a Slack user bearer token. This is the autonomous happy path for the case where the token already exists in Vaultwarden. For the one-time Slack app creation and token mint, see the pointer at the end.

## Core Facts

Slack's hosted MCP endpoint is `https://mcp.slack.com/mcp`. Auth is a Slack user token (`xoxp-...`) sent as a bearer header.

The hosted browser OAuth flow does not complete for the API build of Claude Code. It shows "Failed to connect" or stays in needs-auth and never finishes. Use the bearer token instead. Do not run an OAuth login for Slack in Claude Code.

The token lives in Vaultwarden under the default `ix-infra` folder:

```nu
rbw get --folder ix-infra --field "User Token" "Slack Codex MCP App"
```

Claude Code stores MCP server config in `~/.claude.json`. Keep the token out of that file by storing the header as a literal `${SLACK_MCP_TOKEN}` reference; Claude Code expands it from the environment at startup.

## Autonomous Setup

Run end to end. No human steps when the token is already in Vaultwarden and `SLACK_MCP_TOKEN` is in the environment.

1. Confirm the token is reachable:

```nu
rbw get --folder ix-infra --field "User Token" "Slack Codex MCP App" | str starts-with "xoxp-"
```

2. Ensure `SLACK_MCP_TOKEN` is in the shell environment that launches Claude Code:

```nu
"SLACK_MCP_TOKEN" in $env
```

If it is missing, wire it once through Nushell secrets (see env / Config Placement below), then re-check in a fresh shell.

3. Add the server at user scope with the header kept as a literal env-var reference:

```nu
claude mcp add --transport http --scope user slack https://mcp.slack.com/mcp --header "Authorization: Bearer ${SLACK_MCP_TOKEN}"
```

Re-running is safe. To reset, `claude mcp remove "slack" -s user` then add again.

4. Verify (see Verification). Then note the restart requirement: a Claude Code session loads its MCP tool set at startup, so an already-running session needs a restart before the live `mcp__slack__*` tools appear.

## env / Config Placement

`SLACK_MCP_TOKEN` must be present in the environment when Claude Code starts. On this machine it is wired through Nushell secrets:

- Template: `~/.config/nix/nushell/secrets.template.nu` defines `$env.SLACK_MCP_TOKEN`.
- Rendered cache: `~/.cache/nushell/secrets.nuon`, loaded automatically by `env.nu` in fresh sessions.

For a fresh setup where the var is missing, add this line to the template, matching the existing Vaultwarden ref pattern:

```nu
$env.SLACK_MCP_TOKEN = "{{ bw://ix-infra/Slack Codex MCP App/User Token }}"
```

Then rebuild Home Manager and refresh the secrets cache:

```nu
home-manager switch --flake ~/.config/nix
nu -l -c 'refresh-secrets'
```

## Verification

Probe the endpoint directly to prove the token works independent of the MCP client:

```nu
let token = (rbw get --folder ix-infra --field "User Token" "Slack Codex MCP App")
^curl -sS -i https://mcp.slack.com/mcp \
  -H $"Authorization: Bearer ($token)" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-probe","version":"0"}}}'
```

Success is HTTP 200, `serverInfo.name` equal to `Slack MCP`, and an `mcp-session-id` response header.

Then check Claude Code's view:

```nu
claude mcp get slack
claude mcp list
```

`slack` should report `✓ Connected`. `claude mcp get slack` shows the resolved bearer token because Claude Code expanded `${SLACK_MCP_TOKEN}` at runtime, while `~/.claude.json` still stores the literal reference.

## Identity

This is a shared user token, so anything the agent posts to Slack appears as the token owner (Andrew), not as the person running the agent. For per-person identity, each person mints their own Slack user token with the same scopes from their own Slack app. There is no working OAuth shortcut for the API build of Claude Code.

## Failure Map

- "Failed to connect" or stuck in needs-auth after an OAuth attempt: the browser OAuth flow does not complete for API Claude Code. Remove any OAuth-based slack entry and add the bearer-token entry above.
- `slack` not listed or not `✓ Connected`: confirm the server was added at user scope and that the header value is exactly `Authorization: Bearer ${SLACK_MCP_TOKEN}`.
- Header resolves to an empty or literal `${SLACK_MCP_TOKEN}` at runtime: the env var was not set in the shell that launched Claude Code. Wire it through Nushell secrets, open a fresh shell, then restart Claude Code.
- `mcp__slack__*` tools missing inside a running session: Claude Code reads its MCP tool set once at startup. Restart the session.
- Token rejected by the curl probe (not HTTP 200 with `serverInfo.name` = `Slack MCP`): the Vaultwarden `User Token` is stale or wrong. Re-mint it (see pointer below) and update Vaultwarden.

## One-time app and token mint

The Slack app creation, scopes, redirect URL, and the OAuth token-mint flow that populates the Vaultwarden `User Token` are documented once in the Codex skill `slack-init-mcp` (`.agents/skills/slack-init-mcp/SKILL.md`). Use it only for the rare case where no token exists in Vaultwarden yet; the minted token is shared by both the Codex and Claude Code setups.
