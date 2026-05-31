---
name: user-facing-commands
disclosure: progressive
description: "How CLI output should behave: human vs machine-readable, progress phases, error shape. Use when building or changing a user-facing command."
---

## User-facing commands

Keep protocol emitters separate from product workflow code. Workflows should
produce facts; terminal, API, and document surfaces should render those facts
for their audience.

Human-readable output is the default for interactive commands. Agents, scripts,
and tests should prefer machine-readable output when the command supports it.

Long-running commands should expose the phases users naturally ask about.
Terminal progress should keep moving while work is in flight, with recent rate
and cumulative totals reported as separate facts when both matter.

Default errors should lead with the operator-facing failure and actionable
context. Source locations, backtraces, trace paths, and internal module paths
belong behind debug output or structured output unless they are the user's next
step.

