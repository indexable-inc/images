# pi-harnesses

`packages/pi-harnesses` is a collection of Pi-based agent harnesses. Each harness
is a thin, declarative wrapper around [`pi`](https://pi.dev) with a fixed posture
and one or more bundled extensions, shipped as its own Nix package (`README.md`).
There is no Rust here: the wrappers are `stdenv`/`buildNpmPackage` derivations
around TypeScript/JavaScript extensions.

| sub-dir | id / flake output | what |
| --- | --- | --- |
| `base/` | `pi-base` | base UX pack: live tok/s, git status widget, `/diff`, `/lg`. No agent behavior change. |
| `prosecutor/` | `pi-prosecutor` | executor under a skeptical, context-isolated supervisor with earned-trust check-ins. |
| `beam/` | `pi-beam` | executor that turns a hard decision into a bounded [beam search](beam.md) over isolated worktree branches. |
| `fusion/` | `pi-fusion` | Fable primary agent delegating bounded work to a `gpt-5.6-sol` low sidekick. |

This is ONE component directory; each member package is documented here, with the
beam-search executor on its own page ([beam.md](beam.md)).

## Shared builder (`shared/mk-pi-harness.nix`)

`mk-pi-harness.nix` (`shared/mk-pi-harness.nix:1-147`) wraps `pi` with a
build-time posture and produces a single `bin/<name>` launcher. It is used by
`base`, `prosecutor`, and `beam`. Key arguments:

- `pi` (default nixpkgs `pi-coding-agent`): the bare binary the wrapper execs.
  Pinned so the wrapper never resolves `pi` from the caller's PATH, where a
  host-level wrapper would inject conflicting flags/extensions; override with
  `.override { pi = ...; }`.
- `models`: the alias -> `{provider, model, thinking?}` table; `defaultModel`.
- `extensions` (auto-loaded with `-e`), `libFiles` (helpers copied to
  `share/<name>/lib`), `auxFiles` (copied but not auto-loaded).
- posture knobs: `lockdown` (adds `--no-builtin-tools --no-extensions
  --no-skills`), `session`, `headless`, `mode`, `systemPrompt`, `env`,
  `runtimeInputs`.
- `checkFiles`/`checkLib`: node `--test` files run at build time.

`lockdown = false` is the whole point of the orchestration harnesses: the
executor must do real work and child agents must probe real state. The launcher
template `wrapper.sh.in` resolves the model alias to `PI_PROVIDER`/`PI_MODEL`/
`PI_THINKING` and wires the `-e` extension flags.

## Models and keys (`shared/models.nix`)

One canonical table (`shared/models.nix`): `claude` = anthropic
`claude-opus-4-8` (no thinking level; 4.8 is adaptive-only), `codex` = openai
`gpt-5.6-sol` at `thinking = medium`, `fable` = anthropic `fable-5`, and
`codex-low` = openai `gpt-5.6-sol` at `thinking = low`. Aliases map straight to
`pi --provider --model [--thinking]`. API keys are NOT stored here: each harness
receives them from the caller's environment (`ANTHROPIC_API_KEY`/`OPENAI_API_KEY`)
and Pi reads the named var itself. The harness owns model selection only, not
secret lookup.

Select a model per run with `PI_HARNESS_MODEL`:

```
nix run .#pi-prosecutor -- "your task"               # claude (opus-4-8)
PI_HARNESS_MODEL=codex nix run .#pi-beam -- "..."    # gpt-5.6-sol medium
nix run .#pi-fusion -- "your task"                   # fable primary + gpt-5.6-sol low sidekick
```

## base (`pi-base`)

Pi with a small base UX pack: quality-of-life extensions with no agent behavior
change (`base/README.md`), adapted from `davis7dotsh/my-pi-setup` (MIT). Built via
`mk-pi-harness.nix` with four auto-loaded extensions
(`base/default.nix:17-22`):

- `tps-tracker`: live tokens/sec in the footer while streaming, final per-run
  summary toast.
- `git-status-widget`: ` <branch> · N unstaged files` above the editor,
  refreshed every 2s and on activity.
- `turn-diff`: `/diff`, `/diff list`, `/diff clear` tracking exactly which files
  the last run touched (git baseline + edit/write tool targets); picker opens one
  in `$PI_DIFF_EDITOR`/`$EDITOR`.
- `lg`: `/lg` concise unstaged-change summary with per-file +/- counts.

The extension files also land in `share/pi-base/`, so they can be symlinked into
`~/.pi/agent/extensions/` for the same UX in a plain `pi` session.

## prosecutor (`pi-prosecutor`)

An executor under a skeptical supervisor with earned-trust check-ins
(`prosecutor/README.md`). The executor works with its full tool surface; on an
adaptive interval it is forced to stop and state one falsifiable claim via the
`claim({statement, verify})` tool, and until it does the `tool_call` gate blocks
every other tool (`prosecutor/extension/prosecutor.ts`). The claim goes to a
**prosecutor**: a fresh `pi --print --no-session` process on the same
executor-class model with NO access to the executor's transcript
(`shared/ext-lib/child-agent.js`). It runs the suggested check plus its own probes
and returns `VERDICT: UPHELD` or `VERDICT: BROKEN <evidence>`
(`shared/ext-lib/probes.js`, fails closed on ambiguity).

The asymmetry that matters is context isolation, not a weaker model: no shared
context means the two agents cannot launder each other's hallucinations. Trust is
earned by the `shared/ext-lib/trust.js` state machine (`trust.js:10-30`): start at
interval `min = 1` (verify every action); each upheld claim doubles the interval
up to `max = 16`; a single broken claim snaps it back to 1 and tells the executor
to recover. A competent run accelerates toward autonomy; a confused run gets
babysat harder, with no constant to hand-tune. Config: `PI_HARNESS_MODEL`,
`PI_PROSECUTOR_PROVIDER`/`_MODEL`/`_THINKING` (override the prosecutor model;
defaults to the active executor model), `PI_PROSECUTOR_GOAL` (else captured from
the first user message). Built via `mk-pi-harness.nix` with `lockdown = false`
(`prosecutor/default.nix`); `trust.js` is unit-tested at build time.

## beam (`pi-beam`)

The bounded beam-search executor: see [beam.md](beam.md).

## fusion (`pi-fusion`)

Fusion-style primary/sidekick harness (`fusion/README.md`). The primary defaults
to `fable` and gets a `delegate` tool for bounded sidekick work. The sidekick
defaults to OpenAI `gpt-5.6-sol` with `thinking=low`, runs headless in an isolated
worktree, and returns a summary plus patch for the primary to review/apply.

## Building

Each member ships its own flake output (`package.nix`: `{ id = "pi-<name>";
packageSet = true; flake = true; }`); the registry auto-discovers new harnesses
with no central list to edit.

```
nix run .#pi-base       -- "task"
nix run .#pi-prosecutor -- "task"
nix run .#pi-beam       -- "task"
```
