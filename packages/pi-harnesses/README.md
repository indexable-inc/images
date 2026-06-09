# pi-harnesses

A collection of Pi-based agent harnesses. Each harness is a thin, declarative
wrapper around [`pi`](https://pi.dev) with a fixed posture and (optionally) one
or more bundled extensions, shipped as its own Nix package.

## Layout

```
pi-harnesses/
  shared/
    mk-pi-harness.nix     # the builder: wraps `pi` with flags + extensions + a model table
    wrapper.sh.in         # launcher template (model-alias resolution, -e wiring)
    models.nix            # canonical alias -> { provider, model } table
    ext-lib/              # reusable extension helpers (trust, child-agent, probes, scoring, turn-cap)
  engine/                 # id: pi-harness   - the locked-down Room engine (tools ABSENT, JSON event stream)
  prosecutor/             # id: pi-prosecutor - executor under a skeptical, earned-trust supervisor
  beam/                   # id: pi-beam       - executor with beam search over isolated worktree branches
```

`engine/` is the original `packages/pi-harness` (ENG-2261/2262), moved here
unchanged so the family lives in one place. Its `id` is still `pi-harness`, so
`nix run .#pi-harness` and `index.packages.<sys>.pi-harness` are unaffected. It
keeps its own hardened C launcher for the secret-bearing Room posture; the new
harnesses use the simpler shared shell builder.

## The key difference in posture

The engine deliberately removes the model's tools (`--no-builtin-tools
--no-extensions --no-skills --no-session`) so Room gets a sandboxed, single-shot
engine. The orchestration harnesses need the opposite: the executor must do real
work and the child agents must probe real state.

| | engine (`pi-harness`) | prosecutor / beam |
| --- | --- | --- |
| built-in tools | absent (`--no-builtin-tools`) | present |
| extensions | only the ix-mcp bridge | the harness extension(s) |
| session | `--no-session` (ephemeral) | persistent |
| child agents | none | isolated `pi` subprocesses |
| posture set by | hardened C launcher | `mk-pi-harness.nix` (`lockdown = false`) |

## Building

```
nix run .#pi-prosecutor -- "your task"          # opus-4-8 executor + prosecutor (same model)
nix run .#pi-beam       -- "your task"
PI_HARNESS_MODEL=codex nix run .#pi-prosecutor -- "..."   # gpt-5.5 medium
```

API keys come from the caller's environment (`ANTHROPIC_API_KEY` /
`OPENAI_API_KEY`); the harness owns model *selection* only. `pi` must be on PATH
until the dependency-intake follow-up pins a `pi` derivation (same as engine).

## Adding a harness

1. Create `pi-harnesses/<name>/` with a `package.nix` (`{ id = "pi-<name>";
   packageSet = true; flake = true; }`) and a `default.nix` that calls
   `../shared/mk-pi-harness.nix`.
2. Put the entry extension under `<name>/extension/`, shared helpers in
   `shared/ext-lib/` (passed as `libFiles`).
3. The registry auto-discovers it; no central list to edit.
