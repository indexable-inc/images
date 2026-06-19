---
name: prompt-eval
description: Evaluate whether a prompt or instruction change actually works by spawning a fresh, clean-context Claude agent and checking if it produces the intended behavior. Use when the user edits a CLAUDE.md, a skill, an agent or subagent definition, a system prompt, a tool description, or a memory and wants to verify the change took effect, test that Claude now follows the new rule, A/B a prompt tweak, or confirm the output comes out in the form they wanted. Spawns separate sessions with neutral tasks and reports a pass rate.
---

# Prompt-change evaluation

A prompt change is only "done" when a fresh agent, one that loaded the prompt the same way a real session will, actually behaves the way you intended. This skill is how to prove that. The core move is exactly what it sounds like: spawn a separate Claude in a clean context, hand it a normal task, and watch whether the new behavior emerges on its own.

## The cardinal rules

These are what make the eval valid instead of self-deluding:

1. **Never evaluate in the current session.** Your own context already contains the change (you just made it, or it is loaded), so any in-session check trivially "passes" and proves nothing. The test must run in a *separate process* that re-reads the prompt from disk: `claude -p ...`, which is a real production session (full Claude Code system prompt, the same memory hierarchy, the session model). An Agent-tool subagent is **not** equivalent: per the Claude Code docs it does load `~/.claude/CLAUDE.md` (only the built-in Explore and Plan agents skip it), but it runs its own minimal system prompt rather than the full Claude Code one, and CLAUDE.md reaches it as a soft user message, so it under-applies stylistic/house-style rules and gives false negatives. Dogfooded 2026-06-04: a `general-purpose` subagent on both opus and sonnet ignored a global ✅/❌/🤔 convention that real `claude -p` sessions applied 5/5. So use the Agent tool only to test an agent/subagent *definition* (its own prompt is the thing under test); use `claude -p` for CLAUDE.md, skill, memory, or system-prompt changes.

2. **Apply the change first if it needs a switch.** Global `CLAUDE.md` and global skills are store symlinks: they do not change until `home-manager switch`. If you skip it, the fresh agent reads the OLD prompt and your eval is meaningless. Project `CLAUDE.md` and out-of-store-symlinked files are live, no switch needed. Confirm the on-disk file the fresh agent will read actually contains the change before testing.

3. **Do not lead the witness.** Give the test agent a *neutral, representative task* that a real user would say, one that should organically trigger the new behavior. Do NOT mention the rule, quote it, or ask "do you follow X". That tests recall, not behavior. (Exception: to check that an instruction merely *loaded*, a direct "what does your instruction say about X" is fine, but that is a weaker check than observing the behavior emerge.)

4. **Sample more than once.** Model output is stochastic. One run is noise. Run N (3-5, more for subtle changes) and report the rate, e.g. `4/5`. A change that fires 2/5 of the time is 🤔, not ✅.

5. **A/B to attribute the effect.** "It did X" is weak if it did X before too. Compare against the baseline: run the task against the pre-change prompt (run before applying, keep the old output), or toggle the change (`--append-system-prompt` with vs without, a clean profile vs the real one). The claim you want is "the change caused the difference," not "the output looks fine."

## Procedure

1. **Name the observable.** State the concrete success criterion before running anything: what must the output contain, what form must it take, what must it avoid. If the change produces no observable difference in output, it is not testable, say so.
2. **Pick the trigger task.** A real-sounding prompt that should make the behavior surface naturally. Write it down.
3. **Run the fresh agent N times** (see templates). Capture each raw output as evidence.
4. **Judge each sample** against the criterion (optionally with a separate judge agent for objectivity, see below).
5. **Report** the pass rate with ✅ / ❌ / 🤔 and paste a representative sample so the verdict is backed by an observation, not a vibe.

## Command templates

Default to **Opus 4.8** on every eval run (`--model opus` for `claude -p`, `model: opus` for the Agent tool) unless you are deliberately testing a different tier. Your real sessions run on Opus 4.8, so the eval must too, or you are measuring a different model.

Fresh clean-context run (prompt-file changes, CLAUDE.md / skills / memory):
```
claude -p "<neutral representative task>" --model opus --allowedTools ""
```
- Run from the directory whose project `CLAUDE.md` you changed; global `CLAUDE.md` loads regardless of cwd.
- `--model opus --allowedTools ""` forbids tools so the eval isolates prompt-driven reasoning/output. Drop it (or pass a tight allowlist) only when tool use is the behavior under test.
- Loop it for a rate:
```
for i in $(seq 1 5); do echo "--- run $i ---"; claude -p "<task>" --model opus --allowedTools ""; done
```

A/B a system-prompt-style change:
```
claude -p "<task>" --model opus --allowedTools ""                                  # baseline
claude -p "<task>" --append-system-prompt "<the new instruction>" --model opus --allowedTools ""   # with change
```

Separate judge (objective grading, batchable):
```
claude -p "Criterion: <criterion>. Candidate output below. Answer PASS or FAIL on line 1, one-line reason on line 2.\n\n<candidate>" --model opus --allowedTools ""
```

Agent/subagent-definition changes: spawn via the Agent tool with that `subagent_type`, give it the neutral task, inspect the returned output. (This is the one case where the Agent tool is right, because the definition is what you are testing.)

## Reporting

Use the status-emoji convention:
- ✅ the change reliably produces the intended behavior (high pass rate, attributable to the change)
- ❌ it does not (fails, or the behavior was already there so the change did nothing)
- 🤔 flaky or partial (fires sometimes, or right behavior in the wrong form), with the rate stated

Always include the pass rate and at least one real sampled output. If you could not A/B, say the effect is unattributed.

## Gotchas

- The fresh agent inherits the same global config (model, hooks, other CLAUDE.md sections). That is realistic, but a *different* instruction can mask or fight yours. If a change does not fire, check for a conflicting rule before concluding it failed.
- `claude -p` is a full session and costs tokens/time; keep tasks small and N modest. Background long batches rather than blocking.
- **Match the model, default to Opus 4.8.** Run the eval on the same model your real sessions use, which is Opus 4.8 by default (`--model opus` for `claude -p`, `model: opus` for the Agent tool). Keep baseline and candidate on the same model. A weaker model (or a model mismatch) drops soft conventions and confounds the result, you will blame the prompt for a model effect.
- A change that only affects what a session *sees at startup* (loaded instructions) is validated by a single fresh session reading it back; a change meant to alter *behavior under load* needs the neutral-task + sampling approach.
