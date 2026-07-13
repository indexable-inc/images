---
description: Respond to PR comments with Claude-prefixed replies
argument-hint: <filter>
allowed-tools: Task
---

Launch the pr-responder agent in background to respond to unresolved PR comments.

Filter (optional): $ARGUMENTS

## Behavior

For each unresolved PR comment:

1. **If it's an obvious nit or straightforward fix** (typos, formatting, naming suggestions, small refactors, missing docs, style issues, etc.):
   - Fix it directly in the code
   - Make an atomic commit with a descriptive message
   - Push the commit
   - Respond to the comment with the commit hash that addresses it (e.g., "Fixed in abc1234")
   - Be liberal in what you fix - if there's a reasonable interpretation, just do it

2. **If it's a more fundamental concern** (architectural questions, design decisions, requires discussion, unclear intent, or you disagree):
   - Respond with a thoughtful Claude-prefixed reply explaining the situation
   - Do NOT make changes without clarification

```
Task(subagent_type="pr-responder", run_in_background=true, prompt="Respond to PR comments. Filter: $ARGUMENTS")
```
