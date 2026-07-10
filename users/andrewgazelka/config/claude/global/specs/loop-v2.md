# Loop System v2 Spec

## Overview

Redesign of the loop task management system with:
- **Parallel workers** using git worktrees (unlimited, one per feature)
- **YAML state files** queried with `nu` instead of JSON/jq
- **Knowledge accumulation** to reduce redundant codebase exploration
- **Simpler architecture** with consolidated state

## Goals

1. **Faster iteration** - Parallel workers on independent features
2. **Less context waste** - Knowledge base prevents re-exploration
3. **Simpler state** - Single YAML file, nu queries
4. **Atomic commits** - One feature = one commit

## Non-Goals

- Crash recovery / resume mid-feature (restart from last commit is fine)
- Worker pool limits (spawn as many as needed)
- Complex conflict resolution (workers handle their own rebases)

## Architecture

### Components

```
loop (skill)           - Supervisor: spawns workers, tracks progress
├── task-planner       - Initializes project from spec
└── incremental-coder  - Worker: implements one feature in its worktree
```

### Directory Structure

```
.claude/loop/{name}/
├── spec.md            # Original spec (copied from user's spec file)
├── state.yaml         # Single state file (features + progress)
└── knowledge.md       # Accumulated codebase knowledge
```

### State File Format (state.yaml)

```yaml
meta:
  started: 2024-01-15T10:00:00Z
  total_iterations: 0

features:
  - id: 1
    description: "Add user model with email validation"
    phase: 1
    complexity: low
    depends_on: []
    relevant_files:
      - src/models/user.rs
      - src/db/schema.rs
    status: pending  # pending | in_progress | done | blocked
    blocked_reason: null
    block_count: 0
    worktree: null  # path when in_progress

  - id: 2
    description: "Add authentication endpoints"
    phase: 2
    depends_on: [1]
    status: pending
    # ...

progress:
  done: 0
  blocked: 0
  total: 10
```

### Nu Queries (replacing jq)

```nu
# Count passing features
open state.yaml | get features | where status == "done" | length

# Get next available feature (no deps in progress)
open state.yaml | get features
  | where status == "pending"
  | where {|f| ($f.depends_on | all {|d| (open state.yaml | get features | where id == $d | first | get status) == "done" })}
  | first

# Update feature status
open state.yaml | update features {|fs| $fs | each {|f| if $f.id == 3 { $f | update status "in_progress" } else { $f }}} | save state.yaml
```

## Workflow

### Initialization (`loop myproject.md`)

1. **task-planner** reads spec, extracts features
2. Creates `.claude/loop/{name}/` with:
   - `spec.md` (copy of original)
   - `state.yaml` (features with phases, deps, estimated files)
   - `knowledge.md` (empty, will accumulate)

### Execution Loop

```
SUPERVISOR (loop skill):
  1. Read state.yaml
  2. Find all features where:
     - status == "pending"
     - all depends_on have status == "done"
  3. For each ready feature IN PARALLEL:
     a. Create worktree: git worktree add .worktrees/{name}-feat-{id} main
     b. Update state.yaml: feature.status = "in_progress", feature.worktree = path
     c. Spawn incremental-coder in background:
        - prompt: "continue: {name} --feature {id} --worktree {path}"
     d. Wait for result via TaskOutput
  4. When worker returns:
     - "merged: {id}" → update status=done, delete worktree
     - "blocked: {id}: reason" → increment block_count, maybe mark blocked
  5. Print minimal progress: "▓▓▓░░ 3/5 features (2 active)"
  6. Repeat until all done or all blocked
```

### Worker Flow (incremental-coder)

```
1. ORIENT (fast):
   - Read knowledge.md for codebase patterns
   - Read feature from state.yaml (includes relevant_files hint)
   - Read only the relevant_files listed

2. IMPLEMENT:
   - Write code following patterns from knowledge.md
   - Write tests
   - Commit: git add -A && git commit -m "feat: {description}"

3. MERGE:
   - git fetch origin main
   - git rebase origin/main
   - If conflicts: resolve them, continue rebase
   - git checkout main && git merge {branch} --ff-only
   - git push origin main (or just leave for supervisor if no remote)

4. KNOWLEDGE UPDATE:
   - Append any new learnings to knowledge.md:
     - File purposes discovered
     - Patterns used
     - Gotchas encountered

5. RETURN:
   - "merged: {id}" on success
   - "blocked: {id}: reason" if stuck after 3 attempts
```

### Worktree Management

```bash
# Create (supervisor does this)
git worktree add .worktrees/{name}-feat-{id} main

# Delete after merge (supervisor does this)
git worktree remove .worktrees/{name}-feat-{id}

# List active (for debugging)
git worktree list
```

## Knowledge Base (knowledge.md)

Accumulated by workers, read at start of each worker session:

```markdown
# Codebase Knowledge

## File Map
- `src/db/mod.rs` - Database connection pool, uses sqlx
- `src/models/` - Domain models with validation
- `src/api/` - Axum handlers

## Patterns
- Error handling: `Result<T, AppError>` where AppError impl IntoResponse
- Tests: Integration tests in `tests/`, unit tests inline
- Validation: Use `validator` crate derive macros

## Gotchas
- Must run `sqlx prepare` after schema changes
- The `User` model has a private `password_hash` field - use `User::new()`
```

## Changes from v1

| Aspect | v1 | v2 |
|--------|----|----|
| State files | 3 JSON files | 1 YAML file |
| Queries | jq | nu |
| Parallelism | Sequential | Unlimited parallel via worktrees |
| Context | Re-explore each time | knowledge.md accumulates |
| Commits | PR-sized | Atomic (one feature = one commit) |
| Worker management | Single worker reused | Fresh worktree per feature |
| Conflict handling | N/A (sequential) | Worker rebases and resolves |

## File Changes Required

### @claude/global/skills/loop/SKILL.md
- Add worktree creation/deletion commands
- Change from sequential to parallel spawning
- Switch from JSON/jq to YAML/nu queries
- Simplify output to progress bar style
- Remove checkpoint logic (not needed)

### @claude/global/agents/incremental-coder.md
- Add `--worktree` parameter handling
- Add knowledge.md reading at orient phase
- Add knowledge.md appending after implementation
- Add rebase/merge/conflict-resolution steps
- Change output format to just "merged: {id}" or "blocked: {id}: reason"

### @claude/global/agents/task-planner.md
- Output state.yaml instead of 3 JSON files
- Add `relevant_files` estimation to features
- Create empty knowledge.md

## Open Questions

1. **Remote push strategy** - Should workers push to origin, or just merge locally and let supervisor push periodically?
   - Recommendation: Local merge only, supervisor pushes main after each batch

2. **Worktree location** - `.worktrees/` in repo root, or in `.claude/loop/{name}/worktrees/`?
   - Recommendation: `.worktrees/` at repo root (standard git location)

3. **Knowledge.md growth** - Should there be a max size / summarization?
   - Recommendation: Start simple, add summarization later if needed

## Implementation Order

1. [ ] Update task-planner to output YAML format with relevant_files
2. [ ] Update incremental-coder for worktree + knowledge.md workflow
3. [ ] Update loop skill for parallel spawning with worktrees
4. [ ] Test on a small project
5. [ ] Iterate based on real usage

## Example Session

```
$ claude loop myproject.md

Initializing loop project: myproject
Extracted 8 features across 3 phases

▓░░░░░░░ 0/8 features

Spawning workers for phase 1 features...
  Worker → feat#1 (user model)
  Worker → feat#2 (config module)
  Worker → feat#3 (database setup)

▓▓▓░░░░░ 3/8 features (3 active)

feat#1 merged ✓
feat#2 merged ✓

▓▓▓▓▓░░░ 5/8 features (1 active)

feat#3 merged ✓

Spawning workers for phase 2 features...
  Worker → feat#4 (auth endpoints)
  Worker → feat#5 (user endpoints)

...

▓▓▓▓▓▓▓▓ 8/8 features

✓ Loop complete: myproject
```
