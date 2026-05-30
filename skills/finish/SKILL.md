---
name: finish
description: "Run tasks to completion via incremental agent + judge loop. Usage: /finish <name> [task description to generate tasks.json]"
---

Orchestrate iterative task completion using `.claude/finish/<name>/tasks.json`.

## Input

`/finish <name>` — name identifies the task set. Remaining args are a description to generate tasks from (if `tasks.json` doesn't exist yet).

## tasks.json schema

```json
{
  "tasks": [
    {
      "name": "short-kebab-name",
      "description": "what to implement/fix",
      "status": "todo",
      "feedback": []
    }
  ]
}
```

`status`: `"todo"` | `"in_progress"` | `"done"` | `"stuck"`

## Workflow

### 1. Setup

- Parse `<name>` from skill args (first word).
- If `.claude/finish/<name>/tasks.json` does not exist:
  - Use remaining args as description. Break into small atomic tasks.
  - `mkdir -p .claude/finish/<name>/` and write `tasks.json`.
- Read `tasks.json`.

### 2. Loop

Pick first task with `status: "todo"`. If none remain, go to step 6.

Set `status: "in_progress"`, write `tasks.json`.

### 3. Incremental agent

Spawn the **@incremental** agent with this prompt:

```
Task file: .claude/finish/<name>/tasks.json
Task name: {task.name}
Description: {task.description}
Previous feedback: {task.feedback joined by "\n---\n", or "None"}
```

Wait for completion.

### 4. Judge

Spawn the **@judge** agent with this prompt:

```
Task file: .claude/finish/<name>/tasks.json
Task name: {task.name}
Description: {task.description}
```

Wait for completion.

### 5. Evaluate

- **PASS**: set `task.status = "done"`, write `tasks.json`, go to step 2.
- **FAIL**: append feedback to `task.feedback`, write `tasks.json`. If `task.feedback` has **3+ entries**, set `status: "stuck"` and go to step 2. Otherwise go to step 3.

### 6. Completion

- Report: tasks done, tasks stuck (with their feedback).
- Run the nearest Nix package or check owner for final confirmation.

## Constraints

- Each agent spawn is fresh context (not fork).
- Sequential — one incremental agent at a time.
- Always write `tasks.json` after every status change.
- Judge never makes code changes.
