---
name: beeper
description: "Send and read chat messages on any network Beeper bridges (Signal, WhatsApp, Telegram, Discord, Instagram, X, iMessage) through the Beeper Desktop API on localhost. Use whenever the user wants to message someone on Signal or another non-iMessage network, search chats, or read a conversation. Covers the OAuth PKCE token dance, the URL-encoded chatID gotcha, and delivery confirmation."
---

## Beeper Desktop API

Beeper Desktop serves a REST API at `http://127.0.0.1:23373` while the app
runs. The live spec is at `/v1/spec`. Do not drive the Electron UI with
agent-browser for messaging; the API is faster and does not race the user's
focus.

```sh
TOKEN=$(cat ~/.config/beeper/token)
curl -s -H "Authorization: Bearer $TOKEN" http://127.0.0.1:23373/v1/accounts
```

Accounts list one entry per bridged network (`local-signal_*` is Signal,
`whatsapp`, `telegram`, `discordgo`, `sh-imessage`, ...).

### Finding a chat and reading it

```sh
curl -s -G -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:23373/v1/chats/search --data-urlencode "query=Ax"
```

Take `.items[].id` (not `.chatID`, which is null in search results) plus
`.network` and `.title` to pick the right conversation. The id contains `!`
and `:` and MUST be URL-encoded when used in a path:

```sh
CHAT=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1],safe=''))" "$RAW_ID")
curl -s -G -H "Authorization: Bearer $TOKEN" \
  "http://127.0.0.1:23373/v1/chats/$CHAT/messages" --data-urlencode "limit=20"
```

### Sending

POST `{"text": "..."}` to `/v1/chats/{chatID}/messages`. Markdown is accepted
and converted per network. Build the payload with `jq -n --arg` rather than
interpolating into a JSON string.

```sh
jq -n --arg text "$MSG" '{text:$text}' \
  | curl -s -X POST -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
      "http://127.0.0.1:23373/v1/chats/$CHAT/messages" -d @-
```

A 200 with `pendingMessageID` means queued, not delivered. Confirm by
re-reading the last message (`limit=1`) and checking `isSender == true` with
your text.

Disclose AI authorship in every outbound message: open with something like
"Andrew's AI assistant here (Claude, via Claude Code)". The recipient is a
person expecting a person.

### When the token is stale (401 "Invalid token")

The API mints tokens via OAuth PKCE with in-app approval; there is no
long-lived secret to copy. Re-run the dance and save the result back to
`~/.config/beeper/token`:

1. `POST /oauth/register` with `{"client_name":..., "redirect_uris":
   ["http://127.0.0.1:18742/callback"], "grant_types":["authorization_code"],
   "response_types":["code"], "token_endpoint_auth_method":"none"}`.
2. Start a one-shot localhost listener on the redirect port, then `open` the
   `/oauth/authorize` URL with `scope=read+write` and an S256
   `code_challenge`. Beeper pops an approval dialog; the user clicks once.
3. Exchange the code at `/oauth/token` with the `code_verifier`, write
   `access_token` to `~/.config/beeper/token` (mode 600).

Discovery lives at `/.well-known/oauth-authorization-server` if the endpoints
move.

### Gotchas

- `/v0/*` paths 404: that prefix serves only the MCP transport (`/v0/mcp`).
  All REST is under `/v1/`.
- An unauthenticated request to a real `/v1` route returns 401; a 404 means
  the route is wrong, not the auth.
- `Fleet`/kernel helpers do not cover Beeper; iMessage-specific work has its
  own skill (`imessage`) that sends through Messages.app instead.
