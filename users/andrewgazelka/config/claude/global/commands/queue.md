# Queue Research Task

Queue a background research agent to analyze a task and create a plan.

**Argument:** $ARGUMENTS (the task description)

## Instructions

1. **Generate plan slug**: Create a slug from the task (e.g., "add user auth" → "add-user-auth")

2. **Spawn background agent**: Use the `Task` tool with:
   - `subagent_type`: "queue-researcher"
   - `run_in_background`: true
   - `prompt`:
     ```
     Task: $ARGUMENTS
     Slug: {slug}
     Working directory: {cwd}

     Research this task and create an implementation plan.
     Write the plan to ~/.claude/plans/{slug}.md
     Then append to {cwd}/task.md with the plan path and one-line summary.
     ```

3. **Confirm queued**: Tell the user the task has been queued for research. Do NOT wait for the agent to finish.

## Example

User runs: `/queue add user authentication`

You respond: "Queued research for 'add user authentication' → will write plan to ~/.claude/plans/add-user-authentication.md"

(Background agent explores codebase, writes plan, appends to ./task.md)
