# Pop Highest Priority Task

Read task.md, select the highest priority task, confirm with user, then execute its plan.

## Instructions

1. **Read task.md**: Read `./task.md` in the current directory
   - If it doesn't exist or is empty, tell the user there are no queued tasks

2. **Select highest priority**: Analyze the tasks and choose the most important one based on:
   - Dependencies (tasks that unblock others come first)
   - Complexity (simpler tasks that provide quick wins)
   - Impact (high-value features over minor improvements)
   - Your judgment of what's most valuable to do next

3. **Read the plan**: Read the plan file referenced in the selected task (e.g., `~/.claude/plans/add-user-auth.md`)

4. **Present plan for review**: Show the user:
   - Which task was selected and why
   - Full plan details (architecture, key files, implementation steps, considerations)
   - Ask for confirmation before proceeding
   - User may: approve, reject, request changes, or pick a different task
   - If user wants tweaks, update the plan file accordingly before executing

5. **Remove from task.md**: After user confirms, remove the selected task line from `./task.md`

6. **Execute the plan**: Implement the task following the plan's implementation steps

7. **Report**: When done, summarize what was implemented
