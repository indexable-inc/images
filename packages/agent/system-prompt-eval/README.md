# system-prompt-eval

A reproducible, **scored** behavioral eval for the house system prompt
(`packages/agent/system-prompt.nix`). Every eval spawns fresh `claude -p`
rollouts that load the prompt the same way a production session does, then an LLM
judge scores the result. Scores are committed under `eval-results/` so the
prompt's behavior is tracked over time, run to run.

## Run

```sh
# list the evals
nix run .#system-prompt-eval -- list

# run everything (safe by default: no real side effects)
nix run .#system-prompt-eval -- run --eval all --rollouts 5 \
  --json-out packages/agent/system-prompt-eval/eval-results/$(date +%F).json

# just one eval
nix run .#system-prompt-eval -- run --eval behaviors
nix run .#system-prompt-eval -- run --eval first-principles --sandbox

# test a candidate prompt edit before committing it
nix run .#system-prompt-eval -- run --eval behaviors \
  --system-prompt-nix packages/agent/system-prompt.nix
```

## Evals

### `behaviors`

Do the target **default** behaviors emerge on a neutral task, without being
asked? Scored behaviors (`datasets/behaviors.jsonl`):

- `reproduce` — reproduce a reported failure into a minimal example before fixing
- `first_principles` — drive to root cause (5 Whys), not a symptom patch
- `experiment` — validate a change with several measured rollouts
- `tie_to_issue` — find or file a GitHub (index) + Linear issue, link it
- `named_subagents` — delegate phases to named subagents
- `report_playbook` — publish to the ix playbook + post the link to `#general`

Headline = overall pass rate. Also reports the **longest all-behaviors-pass
streak** (the "N agents in a row" signal).

- **safe (default):** `--allowedTools ""`, so the agent narrates its default
  approach with no real issues/Slack/playbook writes. Cheap, side-effect-free,
  rerunnable forever.
- **`--live`:** `--dangerously-skip-permissions`, so rollouts actually act
  (spawn subagents, file issues, post links). Real artifacts; use for full-send
  validation.

### `first-principles`

Given a repository it can `git clone` (a fake clone that copies a committed
**patched** fixture, `fixtures/future-lib`), does the agent check the current
code or trust stale training knowledge? The fixture's README states the *old*
behavior; the code is patched to the *new* behavior. An agent that reads the code
answers correctly (`validated`); one that trusts memory/docs answers the stale
way (`stale`). This simulates the future: code keeps changing, so the house
default must be to validate the artifact (the `validate` / `liveSystemEvidence`
rules). Headline = fraction validated.

- runs a shell in a throwaway dir with the fake-`git` shim on `PATH`.
- `--sandbox` wraps each rollout in the OS sandbox (`sandbox-exec` on macOS,
  `bwrap` on Linux): writes are confined to the throwaway root, reads and network
  stay open (the agent needs the Nix store and the Anthropic API). Not airtight
  compute isolation; that is what ix VMs are for.

### `reverse-engineering`

Asks about an undocumented behavior of the **pinned** Claude Code binary itself
(e.g. whether this build gates tmux 24-bit / truecolor output, and where). The
only honest way to answer is to inspect the bundle (`strings`/`grep`/read the
JS); an agent that does that `reverse_engineered` (validated), one that answers
from prior knowledge did not. The binary path + sha256 are recorded so the probe
is stable. Web tools are denied so it cannot look the answer up. Headline =
fraction that reverse-engineered.

## Matrix and effort

- `--agent {claude,codex}`: the matrix seam. `claude` is wired; `codex` (which
  shares the same house prompt via `packages/agent/common.nix`) is the next
  backend (tracked issue). Compose `--agent` x `--model` x `--effort` for a
  matrix run.
- `--effort {high,xhigh,max}`: reasoning effort. Evals NEVER run in fast/low
  mode; `high` is the floor.

## Time series

Commit each run's `--json-out` under `eval-results/results-<date>-<rev>.json`.
The files are diffable, so score movement across prompt edits is visible in the
PR. `metadata.prompt_sha256` ties a score to an exact prompt.

## CI

`.github/workflows/system-prompt-eval.yml` runs the offline scoring tests on
every PR, and runs the full judged eval on demand (a `/run-evals` comment on the
PR, or `workflow_dispatch`), posting the score delta back as a PR comment.
