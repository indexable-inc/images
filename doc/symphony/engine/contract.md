# Engine contract: the room-server wire seam

`elixir/lib/symphony_elixir/engine/` is the only part of `elixir/lib/` that names
the room-server HTTP contract. It is the WS-0 seam of the overhaul: the DSL, the
runtime, and the Rust room-server all code against these shapes, so a change here
is a deliberate cross-language change with a golden fixture proving both sides
still agree (`packages/agent/symphony/docs/engine-contract.md:1-7`). The room-server
itself lives in the ix monorepo (`crates/room/src/engine.rs`), not here.

Two modules own the Elixir side:

- **`Engine.Envelope`** (`engine/envelope.ex`) - the typed, validated execution
  spec for one agent node.
- **`Engine.Client`** (`engine/client.ex`) - lowers an envelope + a turn to the
  room-server `TurnRequest`, resolves the target URL, POSTs `/api/agent/turns`,
  and maps the `AgentTurnResponse` back to a runtime result.

## Envelope

`engine/envelope.ex` makes every axis of execution explicit and validated at load; the pre-overhaul code
sniffed the engine from the model name and silently ignored mismatched fields
(`engine/envelope.ex:6-11`). Fields (`engine/envelope.ex:13-32`):

| field | values | notes |
| --- | --- | --- |
| `engine` | `:codex`, `:claude`, `:pi` | explicit, never inferred from the model |
| `model` | string | passed through verbatim (`gpt-5.3-codex`, `claude-opus-4-8`, or the `opus`/`sonnet`/`haiku` aliases) |
| `effort` | `:none :minimal :low :medium :high :xhigh` or `nil` | `nil` lets the engine pick its default |
| `permissions` | `:read_only :workspace_write :danger_full_access` | each adapter lowers it to its native shape |
| `location` | `:local`, `:ixvm`, `{:host, name}`, `{:room, url}` | deployment topology, resolved by `Engine.Client` |

The accessors (`engines/0`, `efforts/0`, `permission_levels/0`, `locations/0`) are
the source of truth the [DSL schema](../dsl/overview.md#schema-vocabulary) reads.
`from_map/1` builds a validated envelope from the parser's raw spec map, rejecting
unknown keys (`engine/envelope.ex:99-117`). `validate/1` defaults a missing
`permissions` to `:workspace_write` and a missing `location` to `:local`
(`engine/envelope.ex:119-140`). The mismatch guard the old code could not express:
a Claude-looking model under `engine: :codex` (or a non-Claude model under
`:claude`) is a load error, not a silent mis-route (`engine/envelope.ex:232-241`);
`:pi` is exempt because its model is a harness alias resolved downstream. The
dynamic-tool surface (`tools`) is deliberately NOT on the envelope: it is a
property of the prompt/skill, not of execution (`engine/envelope.ex:34-35`).

## Casing and tagging

Wire conventions (`packages/agent/symphony/docs/engine-contract.md:31-39`): field names
are camelCase (`turnId`, `runId`); enum bodies carry a `type` tag
(`EngineEventBody`) or a `kind` tag (`TurnOutcome`, `EngineAnswer`); scalar enums
serialize as lowercase/snake_case strings (`engine: "claude"`, `permissions:
"danger_full_access"`).

## TurnRequest (Elixir produces, Rust consumes)

`Engine.Client.request_body/2` lowers a validated envelope + turn to the canonical
`TurnRequest` JSON (`engine/client.ex:129-149`). `effort` is omitted entirely when
unset (the room-server uses serde `skip_serializing_if`, mirrored by `drop_nil`),
and any nil field is dropped. The golden fixture
(`contracts/fixtures/turn_request.json`):

```json
{
  "engine": "codex",
  "model": "gpt-5.3-codex",
  "effort": "medium",
  "permissions": "workspace_write",
  "cwd": "/workspace/run_x/primary",
  "prompt": "write FOO to ./hello.txt and stop",
  "tools": [],
  "runId": "run_x",
  "nodeId": "n0"
}
```

`request_body/2` requires a non-empty `prompt` and `cwd`; a missing `cwd` fails
loudly with `:missing_cwd` rather than running a turn in an unknown directory
(`engine/client.ex:158-162`). The Elixir fixture test asserts `request_body/2`
reproduces this JSON byte-for-byte after a round-trip, so the lowering and the
shared fixture cannot drift (`packages/agent/symphony/docs/engine-contract.md:109-117`).

## EngineEvent (Rust produces, Elixir will consume)

One normalized event for one turn. `EngineEventBody` is the superset Codex emits;
Claude is a subset producer (it self-executes tools under
`--dangerously-skip-permissions`, so it never emits `approvalRequest`/
`toolCallRequest`). Body variants: `turnStarted`, `textDelta`, `reasoningDelta`,
`toolCallStarted`, `toolCallOutput`, `fileChanged`, `statusChanged`, `usage`,
`approvalRequest`, `toolCallRequest`, `turnCompleted`
(`packages/agent/symphony/docs/engine-contract.md:64-77`). Only the Rust side parses the
fixture today; the Elixir `EngineEvent` decoder lands with the streaming client.

## AgentTurnResponse (the synchronous result)

`POST /api/agent/turns` is request/response: the room-server runs the whole turn
and returns its terminal outcome, the `threadId` it assigned, the event count, and
the turn's terminal `usage` totals. `parse_response/1` maps `outcome.kind`:
`"ok"` succeeds, `"error"` carries a message + thread id, `"cancelled"` carries
the thread id (`engine/client.ex:231-249`). `parse_cost/1` maps the camelCase
`usage` to the `IR.Attempt.cost` shape (`usd`, `tokens_in`, `tokens_out`,
`cache_read`, `cache_creation`); a response with no `usage` (an older server)
yields `nil` so the attempt records "unknown" rather than a sham zero, and
`costUsd` is dropped when the engine did not price the turn
(`engine/client.ex:253-269`, `docs/engine-contract.md:103-107`).

## Location resolution

`resolve_base_url/2` is the deployment-topology seam (`engine/client.ex:164-204`):
`:local` and a default resolve to the configured `SYMPHONY_ROOM_SERVER_URL`;
`{:room, url}` is explicit; `:ixvm` and `{:host, _}` resolve to the per-run URL the
run's [`Runtime.Placement`](overview.md#placement) provisioned, looked up by
`run_id` in ETS. A turn submitted without the runtime's `run_id` context fails with
`{:unresolved_location, _}` rather than silently routing to the default server.

## Known limitation

The synchronous path blocks until the turn completes, so `submit_turn/2` sets a
60-minute receive timeout and the caller must run it off the runtime process (the
runtime already schedules each attempt in a monitored task). Approvals and
interrupts are not reachable on this path; use `:danger_full_access` or a
self-executing engine until the streaming client lands (`engine/client.ex:39-50`).

## Golden fixtures (`contracts/fixtures/`)

`contracts/` sits beside `elixir/` (not under it) because the Elixir contract test
resolves it relatively (`packages/agent/symphony/README.md:26`,
`packages/agent/symphony/default.nix:26-29`). The fixtures are shared with the
room-server: `turn_request.json` (both sides assert), `engine_event.json` (Rust
asserts today), and `agent_turn_response.json` (both sides assert). A field rename
fails a check on both sides rather than silently at runtime
(`packages/agent/symphony/docs/engine-contract.md:17-29`).
