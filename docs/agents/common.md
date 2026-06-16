# Agents

Tooling that wraps coding agents (Claude Code, Codex, Pi) for the index fleet:
generate the instruction/skill context an agent reads at session start, run
Claude Code hooks, share what teammates are working on, distill reusable lessons
out of past transcripts, lint the skills the agents load, run small command DAGs,
and wrap Pi into bounded-search and earned-trust executor harnesses. Every unit
here is a developer-facing CLI or a thin Nix-built wrapper around an external
agent binary; none runs in the production serving path.

Read this page first, then the component page for the unit you are touching.

## Units

| unit | kind | flake output | role |
| --- | --- | --- | --- |
| [agents-md](agents-md/overview.md) | Rust crate | `agent-context` | render/check/write the always-on `AGENTS.md` (Codex) and `CLAUDE.md` (Claude) instruction files from Nix-assembled fragments. |
| [claude-hooks](claude-hooks/overview.md) | Rust crate | `claude-hooks` | one binary, three Claude Code hook subcommands: `session-digest`, `worktree-guard`, `prompt-priors`. |
| [claude-stories](claude-stories/overview.md) | Rust crate | `claude-stories` | Instagram-style status-line "stories": teammates' avatars + current work, served peer-to-peer over a Tailscale tailnet. |
| [distiller](distiller/overview.md) | Python (Nix-only) | `distiller` (bin `ix-distiller`) | distill ReasoningBank-style lessons + per-session outcome verdicts from local Claude Code transcripts into corpus parquet slices. |
| [skill-lint](skill-lint/overview.md) | Rust crate | `skill-lint` | lint and autofix `SKILL.md` files with a real YAML frontmatter parser. |
| [dag-runner](dag-runner/overview.md) | Rust crate | `dag-runner` | tiny task runner: a JSON DAG of shell commands run in parallel as deps resolve, with graceful termination and progress. |
| [pi-harnesses](pi-harnesses/overview.md) | Nix-only (TS/JS + C) | `pi-harness`, `pi-base`, `pi-prosecutor`, `pi-beam` | declarative wrappers around `pi`: the locked-down Room engine, a UX pack, an earned-trust prosecutor, and the bounded [beam-search](pi-harnesses/beam.md) executor. |

The Rust crates are workspace members of the root `Cargo.toml` and build through
`ix.cargoUnit.selectBinaryWithTests` (see the nix-lib domain). `distiller` is a
pure-Python package built with `toPythonModule` + `makeWrapper`. The
`pi-harnesses` outputs are not Rust: they are `stdenv`/`buildNpmPackage`
derivations that wrap nixpkgs' pinned `pi-coding-agent`.

## How it fits together

These units are largely independent CLIs, but several share data and seams:

- **The session-start context loop.** The repo's `agent-context/` tree
  (fragments with YAML frontmatter) is assembled by `lib/agent-context` (nix-lib
  domain) into two outputs: an always-on core document and a set of progressive
  skills. [agents-md](agents-md/overview.md) is the Rust CLI that renders, diffs,
  checks, and writes that core to `AGENTS.md`/`CLAUDE.md`; its `agent-context`
  flake wrapper bakes the assembled documents in via `AGENTS_MD_DOCUMENTS`. The
  files are not committed: a `SessionStart` hook (`agent-instructions.sh`) prints
  the core as `additionalContext` and copies the skills into `.claude/skills`.
  [skill-lint](skill-lint/overview.md) is the linter for those skill files (and
  the handwritten ones under `skills/`).
- **Claude Code hook injection.** [claude-hooks](claude-hooks/overview.md) is
  wired into the `packages/claude-code` wrapper (`hooks.nix`, `default.nix`):
  `session-digest` on
  `SessionStart`, `worktree-guard` on `PreToolUse`, `prompt-priors` on
  `UserPromptSubmit`. The hooks consume tool paths via env (`IX_GIT`,
  `IX_SEARCH`) and emit Claude Code's `hookSpecificOutput` JSON.
- **The transcript -> corpus -> search funnel.**
  [distiller](distiller/overview.md) reads `~/.claude/projects/**/*.jsonl`, calls
  `claude -p` headless to distill lessons + judge outcomes, and writes
  `source=distilled_facts` and `source=session_outcomes` parquet slices that ride
  the existing archive -> Iceberg lake -> Mixedbread funnel. The
  `prompt-priors` hook in claude-hooks then surfaces those (and other corpus
  sources) back into new sessions via `IX_SEARCH`.
- **dag-runner is the batch substrate.** [dag-runner](dag-runner/overview.md)
  powers `nix run .#health-checks` and is the planned replacement for
  `ix-fleet`'s sequential loops. It is not specific to agents but lives here as
  general developer task running; see the AGENTS.md "why dag-runner" section.
- **pi-harnesses share one builder.** All four Pi wrappers select models from
  one `models.nix` table (`claude` = opus-4-8, `codex` = gpt-5.5 medium) and take
  API keys only from the caller's env; the engine keeps a hardened C launcher,
  the rest use `shared/mk-pi-harness.nix`.

## Invariants

- **Hooks fail open and silent.** Every claude-hooks subcommand returns with no
  stdout on any missing input, parse error, or kill-switch
  (`packages/claude-hooks/src/main.rs:1-5`): a broken or noisy hook is strictly
  worse than no hook. Kill switches are `CLAUDE_CODE_DISABLE_*` env vars.
- **Instruction files are generated, never committed.** `AGENTS.md`/`CLAUDE.md`
  stay gitignored; `agents-md --check` is the gate that they match the assembled
  source, and the size of the always-on tier is a `nix build` invariant
  (`alwaysCharCap`).
- **Distilled slices obey the corpus contract exactly.** The 9-column schema,
  `sha256:<hex>` body hashes, and `_manifest.json` corpus hash mirror
  `packages/sink/parquet` byte-for-byte, and every slice is re-validated with a
  second parquet reader (polars) before it is trusted
  (`packages/distiller/src/distiller/corpus.py`).
- **Beam/prosecutor judge on ground truth, not on a model's say-so.** beam ranks
  branches by a score command's exit code then diff size
  (`packages/pi-harnesses/shared/ext-lib/scoring.js`); the prosecutor isolates
  context (a fresh `pi --no-session` with no transcript) so two agents cannot
  launder each other's hallucinations.
- **Untrusted peer/transcript input is bounded.** claude-stories rejects
  future-dated or overflowing peer timestamps and refuses to emit non-http(s) or
  control-char URLs as hyperlinks; distiller skips unparseable transcript lines
  and clips every extracted field.

## Glossary

- **agent-context fragment**: a markdown file under `agent-context/sections/`
  with YAML frontmatter (`name`, `disclosure`, `description`). `disclosure:
  always` joins the always-on core; `disclosure: progressive` becomes a skill.
- **always-on core**: the concatenated `always` fragments rendered to
  `AGENTS.md` (Codex) / `CLAUDE.md` (Claude); every session reads it in full.
- **skill**: a `SKILL.md` directory loaded on demand by Claude Code; only its
  `name` + `description` stay always-visible. Linted by skill-lint.
- **hook**: a Claude Code lifecycle callback (`SessionStart`, `PreToolUse`,
  `UserPromptSubmit`) returning a `hookSpecificOutput` JSON object.
- **kill switch**: a `CLAUDE_CODE_DISABLE_*` env var that makes a hook exit
  silently.
- **story**: a `{name, repo, branch, subject, ts, url}` record describing what a
  developer is currently working on; visible for 24h (`TTL_SECS`).
- **lesson / item**: one self-contained ReasoningBank-style fact distilled from
  transcripts, carried with a stable id and merged incrementally.
- **outcome verdict**: an LLM-judged per-session label (`success` / `partial` /
  `failure` / `abandoned`) in the `session_outcomes` slice.
- **corpus slice**: a `host=/user=/source=` parquet directory plus
  `_manifest.json` that the leader fold ingests generically.
- **DAG node**: one entry in a dag-runner spec: an argv `command` plus optional
  `depends_on`, `env`, `timeout_secs`.
- **harness posture**: whether a Pi wrapper removes the model's tools (engine
  lockdown) or leaves them present (prosecutor, beam, base).
- **beam search**: running 2-4 candidate approaches on isolated git worktrees
  under turn + wall-clock budgets, then ranking by ground truth.

## Components

| component | page | what |
| --- | --- | --- |
| agents-md | [agents-md/overview.md](agents-md/overview.md) | render/check/write generated `AGENTS.md` + `CLAUDE.md` from assembled fragments |
| claude-hooks | [claude-hooks/overview.md](claude-hooks/overview.md) | one binary, three fail-open Claude Code hooks |
| claude-stories | [claude-stories/overview.md](claude-stories/overview.md) | status-line teammate stories over a Tailscale tailnet |
| distiller | [distiller/overview.md](distiller/overview.md) | transcripts -> lessons + outcome verdicts -> corpus parquet slices |
| skill-lint | [skill-lint/overview.md](skill-lint/overview.md) | lint + autofix `SKILL.md` frontmatter with a real YAML parser |
| dag-runner | [dag-runner/overview.md](dag-runner/overview.md) | parallel JSON-DAG command runner with progress and graceful termination |
| pi-harnesses | [pi-harnesses/overview.md](pi-harnesses/overview.md) | Pi wrappers: engine, base UX, prosecutor; plus [beam search](pi-harnesses/beam.md) |
