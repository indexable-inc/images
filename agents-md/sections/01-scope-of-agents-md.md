## Scope of AGENTS.md

`AGENTS.md` is for durable working principles. Add guidance here only when it
applies to a class of future changes across the repo, or when it captures an
architecture invariant that would be expensive to rediscover.

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
