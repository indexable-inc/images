---
name: experiment
description: >
  Run a disciplined change → measure → decide experiment loop for any change,
  especially agent/prompt/harness tuning. Use when the user wants to test
  whether a change actually improves something, A/B two approaches, run
  rollouts, tune a prompt/skill/system-prompt, or decide keep-vs-revert with
  evidence rather than vibes. For agent or Claude-performance work, drive the
  real agent through the index TUI Python harness (tui.harness.Claude) so runs
  are live-visible on the dashboard, run several rollouts, compare against a
  baseline, and keep the change only if it measurably wins.
---

# Experiment: change one thing, measure it, keep it only if it wins

Most "improvements" are unverified guesses. This skill is the loop that turns a
guess into a result: **change one thing, roll it out, compare against the
baseline, and keep it only if the evidence says it helped.** It applies to
anything you can observe (a prompt tweak, a config flag, an algorithm swap), and
it is the *required* loop for anything that touches agent or Claude performance,
where outputs are stochastic and "it looks better" is almost always noise.

The discipline is the point. One change at a time, a stated observable, more than
one rollout, and an honest keep-or-revert decision at the end.

## The loop

1. **Hypothesis + observable.** Write down, before touching anything, what you
   expect to improve and *how you will see it*. The observable must be concrete:
   a pass/fail criterion, a count, a latency, a token total, a yes/no on a
   behavior appearing. "Feels smarter" is not an observable. If the change
   produces no observable difference, stop: it is not testable, say so.
2. **Baseline first.** Measure the current behavior *before* the change, with the
   same task and the same number of rollouts you will use after. Keep the raw
   results. Without a baseline you cannot attribute anything.
3. **One change.** Make exactly one change. Two changes at once means you cannot
   say which one mattered (or whether they cancelled).
4. **Roll out.** Run the trial. For anything stochastic (any LLM/agent output),
   one run is noise: run N (3-5, more for subtle effects) and look at the rate,
   not a single sample.
5. **Compare + decide.** Put the after-results next to the baseline. Decide:
   - ✅ **keep**: it reliably wins and the win is attributable to the change.
   - ❌ **revert**: no improvement, or it regressed. Revert now, do not leave a
     change that did not earn its place.
   - 🤔 **inconclusive**: flaky or partial. Report the rate, do not pretend.
6. **Record.** One line: hypothesis, observable, baseline vs after (with rate),
   decision. This is what makes the next experiment cheaper and stops the same
   idea being re-litigated. File an issue if the result is worth a wider audience.

> Sibling skill: **prompt-eval** answers "did the new behavior take effect at
> all" (a fresh agent loads the prompt and the behavior emerges). **experiment**
> answers the next question: "is it actually *better* than what we had." Use
> prompt-eval to confirm the change is live, then experiment to decide if it
> wins. Apply the change (e.g. `home-manager switch` for a global skill/prompt)
> before measuring, or you are testing the old version.

## Testing agents: drive the real TUI, never `claude -p`

When the thing under test is Claude (or Codex) performance, do **not** evaluate
with a headless `claude -p` and do **not** wrangle `tmux`. Use the index TUI
Python harness. It spawns the *real* agent TUI in a PTY, so the session is live
on the web dashboard (`nix run .#tui-dashboard`) exactly like a human's: you and
the user watch the current state, attach, and interrupt. An experiment you can
watch beats a black box you can only diff, and the harness gives clean,
programmatic prompt/await/read so rollouts are a `for`-loop or an `asyncio.gather`.

The harness is Playwright for an agent REPL (`tui.harness`): `Claude.launch()`,
`agent.prompt()`, `agent.run()`, `agent.wait_for_idle()`, `expect(agent)`. It
handles the fiddly bits (onboarding gates, submit races, quiescence-based "turn
done" detection). See `packages/tui-py/README.md` for the full surface.

### One rollout

```python
from tui.harness import Claude

async with await Claude.launch(cwd="/path/to/repo") as agent:
    reply = await agent.run("your representative task here", timeout=180)
    print(reply)                       # the parsed answer
    (await agent.screenshot()).to_html()  # colored screen artifact for the writeup
```

### A/B with N rollouts each (the real shape)

Score every run against the observable, then compare rates. Use a fresh agent
per run so context does not leak between trials. Run them concurrently.

```python
import asyncio
from tui.harness import Claude

TASK = "a neutral task that should surface the behavior"

def scores(reply: str) -> bool:           # YOUR observable, made concrete
    return "expected-marker" in reply

async def one_run(cwd: str) -> bool:
    async with await Claude.launch(cwd=cwd) as agent:
        return scores(await agent.run(TASK, timeout=180))

async def rate(cwd: str, n: int = 5) -> float:
    results = await asyncio.gather(*(one_run(cwd) for _ in range(n)))
    return sum(results) / len(results)

baseline = await rate("/repo-before")     # or: before applying the change
after    = await rate("/repo-after")      # after the one change
print(f"baseline {baseline:.0%} vs after {after:.0%}")
```

A change that moves the rate from 2/5 to 3/5 is 🤔, not ✅: the sample is tiny.
Raise N before you celebrate a small delta.

### Stepwise control + assertions

```python
from tui.harness import Claude, expect

async with await Claude.launch(cwd="/repo") as agent:
    await agent.prompt("do the thing")
    await expect(agent).to_be_idle(timeout=180)      # turn finished
    await expect(agent).to_contain_text("done")      # observable met (auto-retries)
    transcript = await agent.content()               # full session text
```

## Match the conditions to reality

- **Same model as production.** Default to Opus 4.8 (`/model`), the tier real
  sessions use. A weaker model drops soft conventions and confounds the result.
- **Same model on both sides.** Baseline and candidate must run the same model,
  or you are measuring a model effect and blaming the change.
- **Neutral tasks, don't lead the witness.** Use a task a real user would ask
  that should organically trigger the behavior. Do not quote the rule or ask
  "do you follow X" unless you are only checking that an instruction loaded.
- **Isolate the variable.** Fresh agent (or clean cwd) per rollout, one change,
  baseline captured the same way. If you cannot A/B, say the effect is
  unattributed instead of implying causation.

## Reporting

State it plainly with the rate and a real sample:

- ✅ **keep** — reliably better, attributable. Baseline X/N → after Y/N.
- ❌ **revert** — no gain or a regression (and you reverted it).
- 🤔 **inconclusive** — flaky/partial, rate stated, next step named.

Always paste at least one real sampled output so the verdict rests on an
observation, not a vibe. If a promising change came out 🤔, the result is "need
more N" or "need a sharper observable," not "ship it."
