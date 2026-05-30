---
name: betterstack
description: Query and manage Better Stack Uptime (monitors, heartbeats, incidents, status pages) via its REST API. Use when checking ix.dev uptime/monitor status, investigating an incident or missed heartbeat, acknowledging/resolving incidents, or scripting against Better Stack. Ships a bun TypeScript CLI (bs.ts).
---

# Better Stack

Better Stack Uptime monitors `ix.dev` and friends and fires incidents on missed
heartbeats / failed checks. The API is REST + JSON:API; there is **no** public
OpenAPI spec or Postman collection, only human docs.

- Base URL: `https://uptime.betterstack.com/api/v2`
- Auth: `Authorization: Bearer $TOKEN`
- Shape: JSON:API — every resource is `{ id, type, attributes, relationships }`;
  collections paginate via `pagination.next`.
- Docs: https://betterstack.com/docs/uptime/api/

Telemetry/Logs is a **separate** API (`telemetry.betterstack.com`), not covered here.

## CLI (preferred)

`bs.ts` is a single-file [bun](https://bun.sh) client — no build step. Run it from
this skill directory:

```bash
cd .agents/skills/betterstack

bun bs.ts monitors                 # table of all monitors
bun bs.ts monitors --status down   # filter by status
bun bs.ts monitor <id>             # full monitor JSON
bun bs.ts pause <id> | unpause <id>

bun bs.ts heartbeats               # all heartbeats + status (up/down/pending)
bun bs.ts heartbeat <id>

bun bs.ts incidents                # recent incidents (most recent first)
bun bs.ts incident <id>
bun bs.ts ack <id>                 # acknowledge
bun bs.ts resolve <id>            # resolve

bun bs.ts status-pages

# Raw escape hatch for any v2 endpoint not wrapped above:
bun bs.ts get /monitors?page=2
bun bs.ts post /monitors '{"monitor_type":"status","url":"https://x"}'
bun bs.ts patch /monitors/<id> '{"paused":true}'
bun bs.ts delete /monitors/<id>
```

Global flags: `--json` (raw JSON instead of a table), `--all` (follow pagination),
`--token <t>` (override token). `bun bs.ts help` lists everything.

## Token

Resolution order: `--token` → `$BETTERSTACK_API_TOKEN` → Vaultwarden.

Shared team token lives in Vaultwarden (`rbw`, folder `ix-infra`, item
`Better Stack Uptime API`, field `token`). The CLI reads it automatically once
`rbw` is unlocked:

```bash
rbw unlock
bun bs.ts monitors          # token pulled from ix-infra
```

It is an **Uptime API token** (team-scoped). For cross-team or non-Uptime
resources you'd need a Global API token instead.

## Key endpoints

| Method | Path                          | Description                          |
| ------ | ----------------------------- | ------------------------------------ |
| GET    | `/monitors`                   | List monitors                        |
| GET    | `/monitors/{id}`              | Monitor details                      |
| POST   | `/monitors`                   | Create monitor                       |
| PATCH  | `/monitors/{id}`              | Update (e.g. `{"paused":true}`)      |
| GET    | `/heartbeats`                 | List heartbeats (`status`: up/down)  |
| GET    | `/incidents`                  | List incidents                       |
| POST   | `/incidents/{id}/acknowledge` | Acknowledge incident                 |
| POST   | `/incidents/{id}/resolve`     | Resolve incident                     |
| GET    | `/status-pages`               | List status pages                    |

Discover the rest via the docs or the raw `get`/`post` commands.

## Notes

- Incidents and heartbeats are linked: a `Missed heartbeat` incident's
  `relationships.heartbeat` points at the heartbeat that lapsed. Cross-reference
  `bun bs.ts heartbeats` (status `down`) with open incidents.
- Monitor `status` values: `up`, `down`, `paused`, `pending`, `maintenance`.
