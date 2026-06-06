# room-server

`room-server` is the API + WebSocket backend for the multiplayer
thread viewer. It launches `codex app-server`, persists app-server
events in SQLite, and fans out live deltas to connected clients.

The primary client is the Tauri desktop app in
[`packages/room`](../room) — that app bundles the Svelte UI and
talks to this server over HTTP + WebSocket.

## Run The Packaged Server

```sh
nix run .#room-server
```

The packaged server binds `0.0.0.0:8080` (TCP) and, because the Tauri
client speaks only WebTransport, also opens a WebTransport listener on
`0.0.0.0:4433` (UDP) by default. It persists its database next to
`cwd` as `room.db`. Both listeners reach the LAN: set `ROOM_HOST` to
`127.0.0.1` to keep them loopback-only.

Useful environment variables (each also takes a `--flag` equivalent):

| Var              | Purpose                                                               |
| ---------------- | -------------------------------------------------------------------- |
| `ROOM_HOST`      | Address both listeners bind. Defaults to `0.0.0.0`.                  |
| `ROOM_PORT`      | TCP port. Defaults to `8080`.                                         |
| `ROOM_WT_PORT`   | WebTransport UDP port. Defaults to `4433`.                            |
| `ROOM_NO_WT`     | Set to disable the WebTransport listener (HTTP `/api` only).          |
| `ROOM_WT_HOST`   | Hostname advertised in the WebTransport URL. Defaults to `127.0.0.1`. |
| `ROOM_DB`        | Override the sqlite file path.                                        |
| `ROOM_STATE_DIR` | Directory holding `room.db` when `ROOM_DB` is unset.                  |
| `ROOM_SITE_DIR`  | Optional. If set, serves the built Svelte SPA at `/`.                |

A per-run engine host that serves only the HTTP `/api` surface sets
`ROOM_NO_WT` (or passes `--no-wt`) so the many servers sharing one host
do not collide on the fixed WebTransport UDP port.

`ROOM_SITE_DIR` is optional now — the Tauri client bundles its own
copy of the SPA. Point it at a built `dist/` only if you also want a
browser-accessible fallback.

## HTTP Surface

```
POST /api/chat                   submit a user turn to codex app-server
GET  /api/threads                paginated, latest-first thread index
GET  /api/threads/:id            single thread metadata
POST /api/threads/:id/archive    archive a thread
POST /api/threads/:id/goal       set a thread goal
DELETE /api/threads/:id/goal     clear a thread goal
GET  /api/threads/:id/messages   paginated transcript for a thread
GET  /api/loro/state             Loro state metadata
GET  /api/loro/updates           persisted Loro update metadata
GET  /api/loro/snapshot          current Loro snapshot bytes
GET  /api/health                 liveness probe
GET  /ws                         websocket: bootstrap + live deltas
GET  /                           the built Svelte SPA (only when ROOM_SITE_DIR set)
```

`GET /api/threads` supports `?user=`, `?repo=`, `?status=`, `?search=`,
`?limit=` (default 50, max 200), and `?before=<updated_ms>` for cursor
pagination.

## WebSocket Protocol

Server -> client messages, all JSON, discriminated by `"type"`:

| `type`             | Payload                                  | When                          |
| ------------------ | ---------------------------------------- | ----------------------------- |
| `bootstrap`        | `{ threads: Thread[] }` (last 50)        | On connect                    |
| `thread-upsert`    | `{ thread: Thread }`                     | Thread row created or updated |
| `message-append`   | `{ thread_id, message: Message }`        | New message in any thread     |
| `message-update`   | `{ thread_id, message: Message }`        | tool_result attached to call  |
| `thread-archive`   | `{ thread_id }`                          | Reserved for future use       |
| `ping`             | `{}`                                     | Every 20s as a keep-alive     |

Client -> server messages are currently ignored; any inbound frame is
treated as a keep-alive.

## Data Model

`Thread` rows correspond one-to-one with Codex app-server thread ids.
`Message` rows mirror app-server notifications: `user_prompt` for
Room-submitted user input, `assistant_text` for agent messages,
`thinking` for reasoning, and `tool_call` / `tool_result` style rows
for command execution, MCP calls, web search, and file changes.

The `patch` column carries a unified diff for `apply_patch` tool
calls when app-server exposes one. The UI renders it through `@pierre/diffs`.
