---
name: agent-context-fragments
disclosure: progressive
description: "How the agent-context instruction fragments and the generated always-on core work; read before adding or editing durable agent guidance."
---

## Scope of agent context

Agent instructions are for durable working principles. Add guidance only when it
applies to a class of future changes across the repo, or when it captures an
architecture invariant that would be expensive to rediscover.

There are no committed `AGENTS.md` / `CLAUDE.md` files. Each fragment under
`agent-context/sections/` carries frontmatter declaring its tier: `disclosure:
always` joins the small always-on core, and `disclosure: progressive` becomes an
on-demand skill keyed by its `description`. The `agent-instructions.sh`
SessionStart hook renders both live, so editing a fragment changes the next
session with nothing to regenerate. Keep the always-on tier small: its total
size is a build-time invariant (`lib/agent-context.nix`), so prefer
`progressive` unless the rule truly applies to every turn.

The test for a new rule is generality. It should survive the specific feature
that prompted it, apply to the next helper or module with the same shape, and
read more like a design philosophy than a task note. Specific examples are fine
when they sharpen the rule. The example should never be the rule.

Put local facts in the narrowest home that owns them: README files, option
descriptions, generated reference, issue bodies, module docs, or an inline
comment next to the load-bearing line. When a narrow note keeps growing across
features, promote the broad invariant here and leave the local details where
operators will look first.

Before adding durable guidance, search the tree and existing docs first. Facts
that are easy to rediscover with source search, generated reference, PR history,
or a narrow README should stay out of this file.

Each addition should be one or two direct sentences. Name the invariant, owner,
or decision rule, and include a path, command, URL, or external reference only
when it is the durable handle for that rule.
