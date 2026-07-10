---
description: Launch a parallel sub-agent to do this task
argument-hint: <task>
---

# Parallel Sub-Agent

Launch a parallel sub-agent to perform the given task autonomously in the background.

## Task

$ARGUMENTS

## Instructions

Use the Task tool with `subagent_type: "general-purpose"` and `run_in_background: true` to launch a parallel agent.

The prompt should be:

```
Perform the following task autonomously:

$ARGUMENTS

You have full access to all tools. Complete this task independently and report back with results.
```

After launching, inform the user that the agent has been spawned and provide the agent ID so they can check on it later.
