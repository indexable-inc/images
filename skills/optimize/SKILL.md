---
name: optimize
description: "Mine your own Claude Code history with polars to find where past sessions wasted time and drift, then draft the global config changes that prevent the recurrence. Surfaces slow model loops, slow commands and toolchain/CI/build-graph waste, oversized tool results that bloat or trim context, mistakes a human had to correct, and multi-turn flailing. Use when you want to make future sessions faster and more correct, audit your agent's habits, or decide what CLAUDE.md / skill / hook to add. Invoke with /optimize."
disable-model-invocation: true
---

# Optimize

Analyze the user's Claude Code history to make every future session faster and more correct. You review *how the agent worked* across thousands of past sessions, find the recurring failure modes, and turn each into a durable fix to the global config.

The governing question for every episode: **if you were the agent in that session, what was the fastest correct path, where exactly did it diverge, and what one durable rule would have kept it on that path?** A finding that does not end in a concrete, reusable rule has failed.

## Run the analysis (bundled polars library, no uv)

The skill bundles `build_history_df.py` — a small polars library that scans `~/.claude/projects/*/*.jsonl` (~2200 files, ~800 MB) and returns compact aggregate frames, so the raw history stays out of your context. **Run it in the index MCP python session** (polars is preinstalled there — `mcp__index__python_exec`):

```python
import sys; sys.path.insert(0, "${CLAUDE_SKILL_DIR}/assets")
import build_history_df as o
F = o.build_frames(days=45)          # default window; full=True for all history
o.report(F)                           # prints every leaderboard below
o.write_html(F, "~/.claude/optimize/report.html")   # self-contained browser report
# go deeper — these are polars DataFrames, slice freely:
F["df"]      # one row per assistant turn / prompt (model, out_tok, n_tooluse, n_think, speed, ts)
F["bash"]    # one row per timed Bash call (cmd, seconds, is_error)
F["tools"]   # one row per tool_result (tool, label, size, is_error)
```

To avoid re-scanning across questions, cache once: `o.build_frames(...)["df"].write_parquet("~/.claude/optimize/history_rows.parquet")` (and `bash`/`tools`), then `pl.read_parquet(...)` for instant re-slicing. The module also runs standalone for headless use (`python ${CLAUDE_SKILL_DIR}/assets/build_history_df.py --full --out ~/.claude/optimize`) on any interpreter that has polars — this is what the `optimize-scan` portable service runs via `uv`.

Default to the **last ~45 days** (current model family — old transcripts ran retired models and teach nothing actionable). Expand to `full=True` only when a pattern needs the longer baseline.

**Schema gotchas already handled by the library (do not re-derive wrong):** `usage.speed` is a coarse `"fast"` label, not tokens/sec; `usage.iterations` is a list (count, ~always 1); a *real* human prompt is a string-content user entry with a top-level `promptId` (harness messages like `<task-notification>` and compaction summaries are excluded); per-command wall-clock is the `tool_use`→`tool_result` timestamp delta; "tool too big / context trimmed" is detected by result **size**, since the literal word "truncated" in transcripts is mostly tool data, not a harness trim.

## The signals (each row carries session id + evidence)

1. **Slow model loops** — per model, `output_tokens` / `n_tooluse` / wall-clock; episodes where a heavy model burned many high-tool-use, low-`thinking` turns on mechanical work (a script, a batched call, or a faster model would win). Caveat: long wall-clock can be a permission wait or the 600 s timeout, not compute — confirm against the command.
2. **Slow commands & toolchain waste** — rank commands by `seconds × frequency`. For each long pole, ask what makes the *class* slow every time. Big: a CI stage that should be instant on a warm runner, an uncached `nix flake check`, a build worth splitting or reimplementing. Structural: a build/DAG recompiling unchanged inputs, missing eval/artifact cache, the same heavy artifact rebuilt across sessions. Small: `rg` over `grep`, a scoped build over a whole-workspace one, a flag that skips work. Treat anything repeatedly over ~1 minute as a defect to fix at its owner.
3. **Oversized tool results / context bloat** — the `context bloat by tool` and `biggest single results` tables. Large results get trimmed (losing info) or just eat context. The fix is usually scope: `Read` with offset/limit instead of whole-file, Bash piped to `head`/filtered, a `--limit`, a narrower query. Attribute each to the tool that produced it.
4. **Mistakes a human had to correct** — real prompts after assistant tool activity carrying correction language. Capture the assistant action that triggered it; classify the mistake (wrong assumption, scope creep, destructive edit, ignored instruction). These map most directly to CLAUDE.md rules.
5. **Multi-turn flailing** — long autonomous chains (high tool count since the last real prompt) with error clusters before resolution. The durable rule is the early signal the agent should have heeded on turn 2 instead of turn 12.
6. **Repeated cross-session tasks → reusable scripts** — the `recurring tasks across sessions` table groups commands by a canonical form (paths/numbers/hashes blanked) and counts how many *distinct sessions* run each. A task you re-derive in dozens of sessions (a multi-step setup, a lint/build/deploy incantation, a query you keep rewriting) should become one reusable thing: a script, a `just`/Makefile target, a global skill, or a CLI subcommand. Rank by `distinct_sessions × total_seconds` — high session-count means you keep paying the rediscovery and typing cost. Propose the concrete artifact (where it lives, what it wraps) and the cumulative time it reclaims. Ignore trivial one-liners (`ls`, `git status`); target the expensive or multi-step recipes.
7. **Escape hatches & recurring jank → architecture fix** — the `escape hatches / jank` table counts workaround patterns in commands (silenced stderr, `|| true`, `--no-verify`, force pushes, `sleep` timing hacks, retry/poll loops, `--impure`/`--override-input`, `sed -i` patching, sandbox bypass) by distinct sessions. Each recurring one is the agent *compensating* for something a real fix would remove: a `sleep` poll → an event-driven wait (the Monitor tool); pervasive `--no-verify` → hooks too slow to keep; repeated `--override-input`/`sed -i` patching → a missing upstream option or config knob; constant `2>/dev/null` → tooling so noisy its output is worthless. Name the underlying gap and propose the architectural change that *retires* the workaround — not a tidier workaround. (Meta: this skill's own creation hit exactly this — launchd escape hatches in a "portable" service — and the fix was improving the abstraction, not pinning more keys.)

## From findings to durable fixes

Every confirmed finding becomes a proposed artifact, chosen by what prevents the recurrence:
- **Global CLAUDE.md edit** — a behavioral rule/guardrail/default (target `~/.claude/CLAUDE.md`). Short, imperative, in the file's voice.
- **A new global skill** — a repeatable procedure (target `~/.claude/skills/<name>/`).
- **A hook or setting** — something the harness must enforce automatically (target `~/.claude/settings.json`).
- **A reusable script / CLI / `just` target** — for signal 6, the codified version of a repeated task.

Apply against whatever renders this machine's Claude config: if it is managed by a Nix flake (a personal config repo, or the index repo's `skills/` + home modules), edit the flake source that renders the file, not the live symlink, then re-switch; otherwise edit `~/.claude/` directly. Write each as a paste-ready draft tied to its evidence and estimated recurring time saved.

## Surface what needs the human

Separate findings into (a) fixes you can draft mechanically and (b) decisions only the user can make, and **use AskUserQuestion for (b)** rather than guessing. Examples: "Change default X behavior?", "Is this recurring command worth a CI/build change?", "Adopt this as a new always-on CLAUDE.md rule?", "Set up a periodic run (below)?". A wrong global rule costs every future session, so confirm the judgment calls.

## Deliverables and apply policy

1. Write a dated report to `~/.claude/optimize/report-<date>.md`: ranked findings, each with evidence, the counterfactual (fastest path + divergence), and the proposed artifact.
2. Also emit a self-contained **`~/.claude/optimize/report.html`** so the findings open in a browser. The bundled library already writes one from the raw leaderboards via `o.write_html(F, path)`; for the synthesized report, render your ranked findings + the same tables into HTML too (reuse `write_html`'s output as the data section and prepend your synthesis).
3. Put paste-ready drafts under `~/.claude/optimize/drafts/`.
4. Return a tight summary: the highest-leverage fixes and total estimated time reclaimed, plus the explicit human-decision list.

**Do not mutate the live global config on your own** — proposals come from heuristics over fuzzy data and change every future session. Draft, present, apply only what the user approves. When applying an approved skill/setting, run `uvx skillsaw lint --type dot-claude ~/.config/nix/claude/global` (the switch's own gate) before handing back.

## Scheduling / checking back later

On index-managed hosts a token-free `optimize-scan` portable service already refreshes `~/.claude/optimize/` (launchd on macOS, a systemd timer on Linux) so the heavy data is always fresh and `/optimize` synthesis is instant. If a host has no such service and the user wants periodic runs, know the options and their limits:
- **`/loop` and the cron tools (CronCreate)** — session-scoped: fire only while a session is open and idle, and recurring ones auto-expire in days. Good for "watch this for an hour," not durable.
- **claude.ai Routines / Desktop scheduled tasks** — durable, but cloud routines run on a fresh clone with **no access to local `~/.claude` history**, so they cannot do this analysis.
- **A native scheduled unit running the scan** — the durable fit: a launchd agent (macOS) or systemd timer (Linux), declared once via the index `services.portable.*` layer so it renders on both. The token-free scan is safe to run often; reserve an unattended `claude -p "/optimize"` LLM report for a daily/weekly cadence.

## Method discipline

Practice what you preach. Build the frames once and reuse them; don't re-scan 800 MB per question. Reach for the strongest signal first (errors, correction prompts, size, recurrence counts) before reading raw transcripts; read full sessions only to confirm a pattern the aggregates already flagged. Report what you measured; mark estimates as estimates.
