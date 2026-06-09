# pi-harness

The Index-side Pi engine harness (ENG-2262). It runs [Pi](https://pi.dev) as a
Room-facing engine with **the built-in tools absent**, exposing **only** the
`ix-mcp` tool surface, selecting the model declaratively, and emitting a
machine-readable JSON event stream. This is the Pi equivalent of the Claude
"tools removed" posture in `packages/claude-code` (`restrictToTools`), and it is
the first task in the ENG-2261 "Pi Room integration" stack.

## What it is

A declarative wrapper around `pi` plus one bridge extension. You import the
package, you get a `pi-harness` command that launches Pi already locked down.

```
pi-harness "your prompt"                 # claude (opus-4-8), JSON event stream
PI_HARNESS_MODEL=codex pi-harness "..."  # gpt-5.5 via OpenAI
```

## How the lockdown works

| Concern | Mechanism |
| --- | --- |
| Built-ins absent (not denied) | `pi --no-builtin-tools` - bash/read/write/edit never enter the model's context |
| No accidental tools | `--no-extensions --no-skills` (the bridge still loads via explicit `-e`) |
| Only tool surface | `extension/ix-mcp-bridge.ts` runs `ix-mcp serve` over stdio and re-exposes its tools (`python_exec`, `search_*`, `calendar_*`) via `pi.registerTool` |
| Minimal context | `--system-prompt` defaults to a one-line controlled prompt; override with `PI_HARNESS_SYSTEM_PROMPT` |
| Machine-readable stream | `--mode json` (default) via `room_event_mapper.py`; `PI_HARNESS_MODE=text` for direct interactive dev |
| Model selection | `models.nix` table → `--provider`/`--model` |
| API keys | Read by Pi from the env the caller provides (`ANTHROPIC_API_KEY` / `OPENAI_API_KEY`); never looked up here |
| MCP subprocess env | `ix-mcp` receives a scrubbed allowlist; model-provider keys are blocked so `python_exec` cannot read them |
| Parent process env | On Linux, the launcher marks itself non-dumpable before env/model setup, then preloads a tiny hardening library into Pi so both secret-bearing parent processes deny same-UID `/proc/<pid>/environ` reads before MCP starts |

## Design decisions (the ticket's open questions)

- **Pi SDK vs CLI** → CLI subprocess in `--mode json`. Clean OS process boundary
  Room can spawn or call; headless-testable; no Node embedding. The SDK
  (`@earendil-works/pi-coding-agent`) stays a later option.
- **Guaranteeing built-ins are absent** → `--no-builtin-tools` makes them absent,
  not merely denied. The smoke test asserts no `bash`/`read`/`write`/`edit` tool
  appears in the stream.
- **Model/key ownership** → the harness owns model *selection* (`models.nix`);
  the caller owns *keys* (env), matching the ENG-2261 secret-store plan.
- **Session** → `--no-session` (ephemeral per turn) for this first cut. A
  persistent per-conversation `--session <path>` is the durability follow-up
  (ENG-2264).

## Event stream → Room

Pi's `--mode json` emits its own event names. The Room-facing names the parent
ticket wants are produced by a thin mapping layer (this is the harness's core
value):

| Pi event | Room event |
| --- | --- |
| `turn_start` | `turn_started` |
| `message_update` (`assistantMessageEvent.type=text_delta`) | `text_delta` |
| reasoning/thinking delta | `reasoning_delta` |
| `tool_execution_start` | `tool_call_started` |
| `tool_execution_update` / `_end` | `tool_call_output` |
| `tool_execution_*` for `python_exec` rich outputs / live resources | `cell_update` / `resource_update` |
| message/turn usage | `usage` |
| `turn_end` | `turn_completed` |
| error events | `error` |

ENG-2263 adds `room_event_mapper.py`. In JSON mode, the hardened launcher
allocates a per-run `IX_MCP_STORE` when the caller did not provide one, starts
the mapper, and the mapper starts Pi. The mapper combines Pi's JSON lifecycle
events with ix-mcp's SQLite rows, which are the same `Job`, `Cell`, and
`Resource` objects consumed by the existing MCP Svelte dashboard.

The harness does not render cells, tool calls, or TUIs itself. It emits a
machine-readable feed for the Room server to ingest; the Room Svelte/Tauri UI
will render inline jobs/cells and sidebar resources later.

Pi lifecycle/tool events keep the original Pi event under `raw` and map to
stable Room-facing names such as `turn_started`, `text_delta`,
`reasoning_delta`, `tool_call_started`, `tool_call_output`, `usage`,
`turn_completed`, and `error`.

ix-mcp store updates emit the dashboard-shaped payloads directly:

```json
{ "type": "cell_update", "cell_kind": "execution", "job": { "...": "MCP Job" } }
{ "type": "cell_update", "cell_kind": "presentation", "cell": { "...": "MCP Cell" } }
{ "type": "resource_update", "resource": { "...": "MCP Resource" } }
```

Those payloads preserve the MCP dashboard interpretations for source/code,
stdout tail, status, final result, rich mime outputs, bindings, curated cells,
and live HTML resources. `code_html` is intentionally empty in the harness feed;
the MCP UI already falls back to raw `code`, and final Room rendering owns
syntax highlighting.

## Validation

`smoke/run.sh` builds `ix-mcp`, builds `.#pi-harness`, runs one prompt through
the **shipped** `bin/pi-harness`, and asserts: built-ins absent, `python_exec`
exposed, JSON turn events emitted. It needs network + an API key, so run it
yourself:

```
ANTHROPIC_API_KEY=... ./packages/pi-harness/smoke/run.sh
```

The bridge is packaged with `buildNpmPackage`, so the store extension ships its
`node_modules` and Pi resolves `@modelcontextprotocol/sdk` at runtime. Refresh
the pinned dep hash after changing `package-lock.json` with
`nix run nixpkgs#prefetch-npm-deps -- extension/package-lock.json`.

The MCP bridge's env scrubber is covered by `npm test` inside the Nix build.
If an MCP feature needs an extra non-provider environment variable, add it via
`PI_HARNESS_MCP_ENV_ALLOWLIST=NAME`; model-provider keys remain blocked even
when listed there.

The Room event mapper has a pure stdlib test:

```
python3 packages/pi-harness/room_event_mapper_test.py
```

The process hardening is part of the shipped `pi-harness` binary. On Linux,
the launcher hardens itself first, then sets `LD_PRELOAD` for Pi so the
post-`exec` Pi process reapplies the same non-dumpable boundary. The MCP child
still gets the explicit scrubbed env from the bridge and does not inherit
provider keys.

## Follow-ups (intentionally deferred)

- Package `pi` as a pinned nix dependency (dependency-intake) and wire it +
  `ix-mcp` into `default.nix` `runtimeInputs` instead of relying on PATH.
- Run the ENG-2263 live smoke matrix in CI once model-provider test credentials
  are available.
