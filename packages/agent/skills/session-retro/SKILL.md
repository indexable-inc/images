---
name: session-retro
description: "Retrospect the current session at its end and file a GitHub issue for everything improvable. Use when the Stop retro gate asks for it, or when asked to run a session retro, retro the session, mine this session for friction, or file issues for what went wrong this session. Walks what happened (corrected mistakes, denied or guarded tool calls, workarounds, retries, missing structured interfaces, hook noise, stalled watches, anything repeated), routes each to the owning repo, dedupes against open issues, and files concise issues per the issues skill with AI attribution."
---

# session-retro

Turn the session you just finished into GitHub issues. Friction that repeats is
friction that was never captured; capturing it at session end, in the repo that
owns the fix, is how the tooling improves instead of every session relearning
the same lesson. Bias to filing: many small precise issues beat none, and the
user explicitly wants volume. The one hard limit is duplicates: never two issues
for the same root cause.

This is the agent-driven GitHub side of retrospection. The `friction-report`
Stop hook files coarse items to Linear automatically; this skill produces the
higher-quality, deduped, repo-routed GitHub issues.

## Walk the session

Scan this session's own transcript and your memory of it for every moment it
fell short of fully agentic work. Look for:

- **Corrected mistakes**: a wrong assumption you had to walk back, a wrong file
  or command, something the user had to re-explain or correct.
- **Denied or guarded tool calls**: a hook or permission that blocked you, a
  guard message you had to route around.
- **Workarounds**: a `sed`/manual edit reached for because a proper interface
  was missing, a retry loop, a fallback you disliked.
- **Missing structured interfaces**: a tool with no `--json`, output you had to
  scrape, a flag or helper that should exist but does not.
- **Missing context**: a fact that should have been ambient (a doc, a memory, a
  CLAUDE.md line) and cost time to rediscover.
- **Hook noise / stalled watches**: a hook that misfired or over-fired, a
  monitor that never fired, a background watch that stalled.
- **Anything repeated**: the same step, question, or error that recurred within
  the session or across sessions.

Routine iteration, a new user requirement, and stylistic preference are not
friction. Every item must name the specific tool, file, flag, or missing fact;
generic complaints are worthless.

## Route and dedupe

For each candidate:

1. **Decide the owning repo**: the repo that owns the fix, not where the symptom
   showed up. A weak flag on a repo tool, a misleading system-prompt rule, a
   missing memory: each has a clear owner (`indexable-inc/index`,
   `indexable-inc/ix`, or another org repo).
2. **Search open issues first**: `gh search issues --repo <owner>/<repo>
   "<keywords>" --state open` (or `gh issue list --repo <o>/<r> --search
   "<keywords>"`). If a real duplicate exists, do NOT re-file. When you have new
   evidence, comment on the existing issue instead (`gh issue comment`).

## File

File each survivor per the `issues` skill: short body (problem, evidence,
proposed fix), one concrete observation per issue, labels at filing time
(`bug`, `enhancement`, `documentation`, `ai-capable`, ...). Pass the body
through `--body-file -` or a temp file, never an escaped `--body` string.

Every issue must include:

- **Evidence**: the decisive moment, quoted or with a concrete handle (a
  command that was denied, the exact error, the tool and flag that was missing).
  A repro command or step list when it is a bug.
- **Proposed fix**: the smallest concrete change that would have prevented it.
- **AI attribution**: note it was filed by an AI agent via a session retro (see
  the `issues` and system-prompt AI-disclosure conventions).

Do not open a second issue for a root cause already tracked or already filed
earlier in this same retro.
