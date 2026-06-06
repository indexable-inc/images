# Engine contract

This is the source of truth for the wire shapes shared between the Elixir
runtime and the Rust room-server. It is the WS-0 seam of the overhaul: the
DSL, the runtime, and the room-server all code against these shapes, so a
change here is a deliberate cross-language change with a golden fixture to
prove both sides still agree.

Two layers own these shapes:

- `crates/room/src/engine.rs` (Rust, in the IX monorepo): `TurnRequest`,
  `EngineEvent`, `TurnStatus`, `EngineAnswer`, and the `Engine` trait.
- `elixir/lib/symphony_elixir/engine/` and `ir/` (Elixir): the
  `Engine.Envelope` that lowers to a `TurnRequest`, and the `IR.*`
  durable run state that the runtime persists.

Golden fixtures live in `contracts/fixtures/`. `turn_request.json` is the
shape Elixir produces and Rust consumes, so both sides assert it: the Rust
test in `crates/room/tests/engine_contract.rs` (in the IX monorepo) and the Elixir test
in `elixir/test/symphony_elixir/engine/contract_fixtures_test.exs`. A field
rename fails a check on both sides rather than silently at runtime.
`engine_event.json` is the shape Rust produces and Elixir will consume; only
the Rust side parses it today, because the Elixir `EngineEvent` decoder
lands with the streaming client (the synchronous `/api/agent/turns` path
returns an `AgentTurnResponse`, not an event stream).
`agent_turn_response.json` is the synchronous turn result Rust produces and
Elixir consumes, so both sides assert it: Rust deserializes the fixture and
Elixir feeds it through `Engine.Client.submit_turn/3` and checks the lowered
`cost`.

## Casing and tagging

- Field names are camelCase on the wire (`turnId`, `runId`), matching the
  existing room-server JSON.
- Enum bodies carry a `type` tag (`EngineEventBody`) or a `kind` tag
  (`TurnOutcome`, `EngineAnswer`).
- Scalar enums serialize as a lowercase or snake_case string
  (`engine: "claude"`, `permissions: "danger_full_access"`).

## TurnRequest

The engine-agnostic turn the Elixir `Engine.Client` submits. The room-server
adapter lowers it to engine-native flags.

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

`effort` is omitted entirely when the envelope leaves it unset (the engine
picks its default). `permissions` is one of `read_only`, `workspace_write`,
`danger_full_access`; each adapter lowers it (Codex to sandbox + approval
policy, Claude to a permission mode or `--dangerously-skip-permissions`).

## EngineEvent

One normalized event for one turn. `EngineEventBody` is the superset of
what Codex emits; Claude is a subset producer and simply never emits
`approvalRequest` or `toolCallRequest` (it self-executes its tools under
`--dangerously-skip-permissions`).

```json
{ "turnId": "thread_abc", "seq": 7, "body": { "type": "textDelta", "text": "hello" } }
```

Body variants: `turnStarted`, `textDelta`, `reasoningDelta`,
`toolCallStarted`, `toolCallOutput`, `fileChanged`, `statusChanged`,
`usage`, `approvalRequest`, `toolCallRequest`, `turnCompleted`.

## AgentTurnResponse

The synchronous result of `POST /api/agent/turns`. The room-server awaits
the whole turn and returns its terminal outcome, the thread id it assigned,
the event count, and the turn's terminal `usage` totals. Both engines emit
cumulative `Usage` events, so the response carries the last one as the
whole-turn total; `Engine.Client` lowers it to the `IR.Attempt.cost` shape
(`usd`, `tokens_in`, `tokens_out`, `cache_read`, `cache_creation`).

```json
{
  "threadId": "thread_abc",
  "outcome": { "kind": "ok" },
  "eventCount": 4,
  "usage": {
    "tokensIn": 1200,
    "tokensOut": 340,
    "cacheRead": 800,
    "cacheCreation": 64,
    "costUsd": 0.0123
  }
}
```

`usage` is always present (a turn that emitted none serializes a zeroed
total); `costUsd` is omitted when the engine did not price the turn, so a
present `usd` always means a real number. A response with no `usage` (an
older server) lowers to a nil cost so the attempt records "unknown" rather
than a sham zero.

## Envelope to TurnRequest

`Engine.Envelope` (Elixir) is the authored, validated shape; `Engine.Client`
lowers it to a `TurnRequest` (`request_body/2`). The envelope adds `location`
(`:local`, `:ixvm`, `{:host, name}`, `{:room, url}`), which the client
resolves to the room-server URL and does not put on the wire. The Elixir
fixture test asserts `request_body/2` reproduces `turn_request.json`
byte-for-byte after a JSON round-trip, so the lowering and the shared
fixture cannot drift apart.
