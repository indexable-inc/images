---
name: linear
description: "Use Linear: query, create, and update issues, sub-issues, and comments via the GraphQL API"
---

Work with Linear issues using the GraphQL API at `https://api.linear.app/graphql`. Auth via `LINEAR_API_KEY` env var (`Authorization: <API_KEY>`). If `LINEAR_API_KEY` is not set, stop immediately and tell the user.

## Output

Always return a summary of what you did, including:
- The issue tag (e.g. `ENG-123`)
- The action taken (found existing, created new, moved to In Progress, already In Progress, etc.)
- The issue title and URL

## Team routing

- `Engineering` (key `ENG`) for product/engineering/platform/reliability/dev-workflow work.
- `Admin` (key `ADM`) for admin/ops/finance/legal/people/internal-business work.
- Default to `Engineering`.

## Issue creation

- Apply the smallest accurate existing label set up front: `bug`, `feat`, `security`, `rfc`, `perf`.
- **Title style**: plain English, no conventional-commit prefixes (`fix(scope):`, `feat:`, etc.).
  Describe the problem or outcome, not the commit. Capitalize naturally.
  Good: "Virtiofs device stops receiving kicks after golden restore"
  Bad: "fix(snix-virtiofs): ioeventfd kick regression post-restore (H4, M3)"
  Drop ticket/code references from titles — they belong in the body or as Linear relations.
- Issue body: short problem-first; default shape `problem -> why it matters -> goal`.
- **Keep tickets general unless the user gives explicit implementation details.** State the problem and desired outcome. Do not prescribe wire formats, buffer sizes, protocol specifics, or architecture choices. Let the assignee design the solution.
- Use a small Mermaid diagram when it materially clarifies flow, ownership, or before/after shape.
- RFC/explore issues describe the question and decision to make, not a locked design.
- Detailed mechanisms/phasing belong in follow-up design docs or implementation tickets.
- Optional closing section: `## Potential implementations` for brief non-binding options.

## Sub-issues

- **Create sub-issues up front** when planning work. Use `issueCreate` with `parentId`. Sub-issues should be small, concrete, and checkable (e.g. "Rename Rust config fields", "Update Nix configs", "Add DB migration").
- **When you start working on a sub-issue**, move it `Todo` -> `In Progress` and assign it to yourself. Use `{ viewer { id } }` to get your user ID, then `issueUpdate` with `stateId` (In Progress) and `assigneeId`.
- **When you finish a sub-issue**, move it to `Done` immediately — don't batch them up.
- The parent issue stays `In Progress` until all sub-issues are done and the final commit lands.

## Progress updates

- **Post comments** at natural milestones: plan approved, major phase complete, blocked, unblocked. Keep them short.
- **Update issue description** if the scope changes or the approach shifts during implementation.

## Closing issues

- Repo-changing work closes via commit + push with Linear magic words: terminal change `Fixes ENG-123`; non-terminal change `Part of ENG-123`. Do not mark such issues `Done` manually via MCP.
- Push is the canonical done signal for repo work.
- For non-git work, MCP state changes are allowed.

## Comments

- AI-authored Linear comments must start with a model tag: `[<Model> <version>]`, e.g. `[Opus 4.7]`, `[GPT 5.5]`, `[Sonnet 4.6]`. Use the exact model and version that authored the comment so future readers can weigh the source. Do not use the bare `[AI]` prefix.
- In MCP-authored content, do not rely on `@ISSUE-ID`. Use structured relations or Markdown links.

## API notes

- GraphQL may return HTTP `200` with partial `data` plus `errors`; always check `errors`.
- Prefer direct queries over guessed fields; re-introspect when schema validation/deprecation fails.

## GraphQL reference

Endpoint: `https://api.linear.app/graphql` (POST, `Content-Type: application/json`, `Authorization: <API_KEY>`).
The API supports introspection (`{ __type(name: "...") { ... } }`) for discovering fields not listed here.

### Hardcoded IDs (Engineering team)

| Entity | Name | ID |
|--------|------|----|
| Team | Engineering | `a8845362-21c7-4283-ba80-cea987a3ee74` |
| State | Backlog | `26e788c2-9ed4-43d0-b6a8-d3f9f38ca082` |
| State | Todo | `abf5a9c4-f5fb-4756-a4fa-4680c93c1258` |
| State | In Progress | `052c5340-8e31-411f-8ed7-19c6107dddcc` |
| State | In Review | `77096884-9255-4577-9453-06de496fbce8` |
| State | Done | `180bb996-25b1-4ee0-8edf-091b093216f5` |
| State | Canceled | `12115e09-d086-4a02-85ad-aed313bb8154` |
| State | Duplicate | `95145256-684c-4067-a1ab-1520f44806cf` |

### Common queries

```graphql
# Current user
{ viewer { id name email } }

# List teams
{ teams { nodes { id key name } } }

# Active cycle for a team
{ team(id: "...") { activeCycle { id number startsAt endsAt } } }

# All cycles (with filter)
{ team(id: "...") { cycles(filter: { isActive: { eq: true } }) { nodes { id number startsAt endsAt } } } }

# Team workflow states
{ team(id: "...") { states { nodes { id name type } } } }

# Issues in a cycle
{ cycle(id: "...") { issues { nodes { id identifier title state { name } assignee { id name } } } } }

# Issues by team + filters (cycle, state, assignee, labels, priority, etc.)
{ team(id: "...") {
    issues(filter: {
      cycle: { number: { eq: N } }
      state: { name: { eq: "In Progress" } }
    }) {
      nodes { id identifier title assignee { id name } updatedAt }
    }
  }
}

# Single issue by identifier (e.g. "ENG-123")
{ issue(id: "ENG-123") { id identifier title description state { name } assignee { name } cycle { number } labels { nodes { name } } } }

# Search issues
{ searchIssues(query: "search text", first: 10) { nodes { id identifier title } } }
```

### Issue fields (read)

`id`, `identifier` (e.g. "ENG-123"), `title`, `description`, `number`, `priority`, `estimate`,
`state { id name type }`, `assignee { id name }`, `team { id name }`, `cycle { id number }`,
`project { id name }`, `parent { id identifier }`, `labels { nodes { id name } }`,
`creator { name }`, `createdAt`, `updatedAt`, `url`, `branchName`, `dueDate`,
`children { nodes { ... } }`, `comments { nodes { id body user { name } } }`.

### Mutations

```graphql
# Create issue
mutation { issueCreate(input: {
  teamId: "..."          # required
  title: "..."           # required
  description: "..."
  stateId: "..."         # workflow state ID
  cycleId: "..."         # cycle ID
  assigneeId: "..."      # user ID
  parentId: "..."        # parent issue ID (creates sub-issue)
  priority: 1            # 0=none, 1=urgent, 2=high, 3=medium, 4=low
  labelIds: ["..."]
  projectId: "..."
  dueDate: "2026-03-30"  # TimelessDate
}) { success issue { id identifier url } } }

# Update issue (all fields optional)
mutation { issueUpdate(id: "...", input: {
  title: "..."
  description: "..."
  stateId: "..."
  assigneeId: "..."
  cycleId: "..."
  priority: 2
  labelIds: ["..."]
  addedLabelIds: ["..."]
  removedLabelIds: ["..."]
  projectId: "..."
  parentId: "..."
  dueDate: "2026-03-30"
}) { success issue { id identifier title state { name } } } }

# Batch update issues
mutation { issueBatchUpdate(ids: ["...", "..."], input: { stateId: "..." }) { success issues { id identifier } } }

# Delete / archive issue
mutation { issueDelete(id: "...") { success } }
mutation { issueArchive(id: "...") { success } }

# Create comment
mutation { commentCreate(input: { issueId: "...", body: "..." }) { success comment { id } } }

# Add/remove labels
mutation { issueAddLabel(id: "...", labelId: "...") { success } }
mutation { issueRemoveLabel(id: "...", labelId: "...") { success } }

# Create issue relation (blocks, duplicates, relates)
mutation { issueRelationCreate(input: { issueId: "...", relatedIssueId: "...", type: blocks }) { success } }
```

### IssueFilter fields

Filters use comparator objects. Common patterns:
- String: `{ eq: "value" }`, `{ contains: "text" }`, `{ in: ["a","b"] }`
- Number: `{ eq: 1 }`, `{ gt: 0 }`, `{ lt: 5 }`
- Date: `{ gt: "2026-03-01" }`, `{ lt: "2026-04-01" }`
- Nullable: `{ null: true }` / `{ null: false }`
- Relations: `{ id: { eq: "..." } }` or `{ name: { eq: "..." } }`
- Boolean combinators: `and: [...]`, `or: [...]`

Available filter fields: `id`, `title`, `description`, `number`, `priority`, `estimate`,
`state`, `assignee`, `creator`, `team`, `cycle`, `project`, `labels`, `parent`,
`createdAt`, `updatedAt`, `completedAt`, `canceledAt`, `dueDate`, `snoozedUntilAt`.

### Error handling

GraphQL returns HTTP 200 even on partial failure. Always check for `errors` array in the response.
Rate limits return HTTP 429 — back off and retry.
