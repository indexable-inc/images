---
name: slack-init-mcp
description: Initialize, authenticate, verify, and debug Slack's hosted Model Context Protocol server for Codex. Use when the user asks to install/add/setup/init Slack MCP in Codex, login to Slack MCP, create a Slack app for MCP, wire Slack tokens through Vaultwarden or Nushell, or troubleshoot Codex Slack MCP startup errors such as Dynamic client registration not supported, redirect_uri mismatch, No scopes requested, localhost refused to connect, app not enabled for Slack MCP server access, or JsonRpcMessage deserialize errors during initialize.
---

# Slack Init MCP

## Core Facts

Slack's hosted MCP endpoint is:

```toml
[mcp_servers.slack]
url = "https://mcp.slack.com/mcp"
bearer_token_env_var = "SLACK_MCP_TOKEN"
```

Do not use `codex mcp login slack` for Slack's hosted endpoint unless Codex has explicit pre-registered OAuth client support. Slack does not support Dynamic Client Registration, so generic Codex OAuth login can fail with `Dynamic client registration not supported`.

Use a Slack user token as a bearer token instead. Store it in Vaultwarden and expose it to Codex through the shell environment.

## Config Placement

On this machine, Codex config is Nix/Home Manager owned:

- Source: `~/.config/nix/codex/config.toml`
- Linked target: `~/.codex/config.toml`

Edit the source file, not the Nix-store symlink target. After edits, run:

```nu
home-manager switch --flake ~/.config/nix
```

For normal shell availability of `SLACK_MCP_TOKEN`, add a Vaultwarden template entry to:

```text
~/.config/nix/nushell/secrets.template.nu
```

Example:

```nu
$env.SLACK_MCP_TOKEN = "{{ bw://ix-infra/Slack Codex MCP App/User Token }}"
```

Then rebuild and refresh the cache:

```nu
home-manager switch --flake ~/.config/nix
nu -l -c 'refresh-secrets'
```

Fresh Nushell sessions load `~/.cache/nushell/secrets.nuon` automatically from `env.nu`.

## Slack App Setup

Create the Slack app at:

```text
https://api.slack.com/apps
```

Prefer **From an app manifest**. A minimal manifest:

```yaml
display_information:
  name: Codex MCP
oauth_config:
  scopes:
    user:
      - search:read.public
      - search:read.private
      - search:read.mpim
      - search:read.im
      - search:read.files
      - files:read
      - channels:history
      - groups:history
      - mpim:history
      - im:history
      - channels:read
      - groups:read
      - mpim:read
      - users:read
      - users:read.email
      - emoji:read
      - chat:write
settings:
  org_deploy_enabled: false
  socket_mode_enabled: false
  token_rotation_enabled: false
```

After creating the app:

1. Save the app credentials in Vaultwarden. Use an item with fields `App ID`, `Client ID`, `Client Secret`, `Signing Secret`, `Verification Token`, and later `User Token`.
2. In **OAuth & Permissions**, add and save this redirect URL exactly:

```text
http://localhost:8080/callback
```

3. Enable Slack MCP / app assistant access at:

```text
https://api.slack.com/apps/<APP_ID>/app-assistant
```

Only internal apps or Marketplace-published apps may use Slack MCP. Unlisted public apps are not allowed.

## OAuth Token Flow

Use Slack's user-centric OAuth endpoint. For `oauth/v2_user/authorize`, scopes go in `scope=`, not `user_scope=`.

Construct the authorize URL:

```text
https://slack.com/oauth/v2_user/authorize?client_id=<CLIENT_ID>&scope=search:read.public,search:read.private,search:read.mpim,search:read.im,search:read.files,files:read,channels:history,groups:history,mpim:history,im:history,channels:read,groups:read,mpim:read,users:read,users:read.email,emoji:read,chat:write&redirect_uri=http://localhost:8080/callback
```

After approval, the browser may show `localhost refused to connect`. That is fine. Copy the `code=...` value from the address bar.

Exchange the code:

```nu
let client_id = (rbw get --folder ix-infra --field "Client ID" "Slack Codex MCP App")
let client_secret = (rbw get --folder ix-infra --field "Client Secret" "Slack Codex MCP App")
let code = "PASTE_CODE_HERE"

^curl -sS -X POST https://slack.com/api/oauth.v2.user.access \
  -H "Content-Type: application/x-www-form-urlencoded" \
  --data-urlencode $"client_id=($client_id)" \
  --data-urlencode $"client_secret=($client_secret)" \
  --data-urlencode $"code=($code)" \
  --data-urlencode "redirect_uri=http://localhost:8080/callback"
```

Save the returned `access_token` as:

```nu
Update the `User Token` field on the `Slack Codex MCP App` item in Vaultwarden with the returned `xoxp-...` token.
```

## Verification

First verify the shell env:

```nu
$env.SLACK_MCP_TOKEN | str starts-with "xoxp-"
```

Then verify Slack MCP directly:

```nu
let token = (rbw get --folder ix-infra --field "User Token" "Slack Codex MCP App")
^curl -sS -i https://mcp.slack.com/mcp \
  -H $"Authorization: Bearer ($token)" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"codex-probe","version":"0"}}}'
```

Success returns HTTP 200 with `serverInfo.name` equal to `Slack MCP` and an `mcp-session-id` header.

Finally start Codex from a fresh Nushell and inspect MCP tools:

```nu
codex --yolo
```

Inside Codex:

```text
/mcp
```

## Failure Map

- `Dynamic client registration not supported`: do not use `codex mcp login slack`; configure `bearer_token_env_var` and provide a user token.
- `redirect_uri did not match any configured URIs`: add `http://localhost:8080/callback` under Slack app **OAuth & Permissions**, click **Save URLs**, and use the same URI in both authorize and token exchange requests.
- `Invalid permissions requested` / `No scopes requested`: using the wrong scope parameter. `oauth/v2_user/authorize` needs `scope=...`; `oauth/v2/authorize` uses `user_scope=...` for user scopes.
- `localhost refused to connect`: expected after Slack redirects to the local callback without a listener. Copy the `code=...` from the browser URL.
- `App is not enabled for Slack MCP server access`: enable it at `https://api.slack.com/apps/<APP_ID>/app-assistant`.
- Codex `JsonRpcMessage` deserialize error during initialize: probe the endpoint with `curl`; Slack may be returning a JSON-RPC error that Codex renders poorly. Fix the Slack-side error first.
