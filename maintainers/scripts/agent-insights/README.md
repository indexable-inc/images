# agent-insights

Graph how often you interrupt your local CLI coding agents over time.

`agent-insights.py` reads the session transcripts Claude Code and Codex already
write to disk, and renders a self-contained SVG (no dependencies) plus a text
summary.

- Claude Code: `~/.claude/projects/**/<session>.jsonl`
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`

## Metric

A **turn** is one bout of the agent running after you hand it work. An
**interrupt** is you cutting that bout off mid-flight.

- Codex: a turn is `task_started` to `task_complete` or `turn_aborted`; an
  interrupt is `turn_aborted` with `reason == "interrupted"` (exact timestamps).
- Claude: a turn runs from a human prompt to the last agent activity before your
  next input; an interrupt is a `[Request interrupted by user]` user message.
  Tool results also arrive as `user` messages and are excluded from prompts.

The default graph is the weekly trend of **interrupts per hour of agent run
time**, split Claude vs Codex.

## Data quirks handled

- Codex replays the full parent transcript into every resumed/forked rollout
  file, re-stamping those events at the resume instant. Only events more than a
  few seconds after a file's `session_meta` timestamp are counted, which dedupes
  resumes and drops the fake near-zero interrupt durations the replay injects.
- Claude session files are self-contained (no cross-file message duplication),
  so they are counted as-is. Subagent sidechains are skipped.

## Usage

```
python3 agent-insights.py                 # writes agent-insights.svg, prints summary
python3 agent-insights.py --out /tmp/i.svg
python3 agent-insights.py --since 2026-01-01
python3 agent-insights.py --tool codex
python3 agent-insights.py --no-svg        # text summary only
```

Override transcript locations with `--claude-dir` and `--codex-dir`. Open the
SVG in any browser, or render it: `rsvg-convert agent-insights.svg -o out.png`.
