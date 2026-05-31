# Agent context

This directory owns the agent instructions and skills delivered to a coding
agent at session start. There are no committed `AGENTS.md` / `CLAUDE.md` files:
the content is rendered live by the `agent-instructions.sh` SessionStart hook
(`.claude/hooks/`), which builds the flake packages below and prints the
always-on core as `additionalContext` while repointing `.claude/skills` at the
generated skill link farm. Editing a fragment changes what the next session
sees, with nothing to regenerate or commit.

## Disclosure tiers

Each file in [`sections/`](sections) is one fragment with YAML frontmatter:

```yaml
---
name: rust-style
disclosure: progressive   # always | progressive
description: "Rust house style ... Use when writing or reviewing Rust."
---
## Rust style
...
```

- `disclosure: always` fragments are concatenated into one small always-on
  document that every session reads in full. Keep this tier short: the total
  size is a build-time invariant ([`lib/agent-context.nix`](../lib/agent-context.nix)
  `alwaysCharCap`), so marking too much `always` fails `nix build` instead of
  silently overflowing Claude Code's per-value context cap.
- `disclosure: progressive` fragments each become a Claude Code skill. Only the
  `name` + `description` stay always-visible; the body loads on demand when the
  skill is invoked. `description` is the trigger Claude uses to decide when to
  load it, so write it as "what this covers; use when ...".

## Preview

```sh
nix build .#claude-md --no-link --print-out-paths | xargs cat   # always-on core (CLAUDE.md)
nix build .#codex-md  --no-link --print-out-paths | xargs cat   # always-on core (AGENTS.md)
nix build .#skills    --no-link --print-out-paths | xargs ls    # handwritten + generated skills
```

`nix run .#agent-context -- --write` writes the always-on core to disk as a
contributor convenience; the files stay gitignored.

## Consuming from another repository

`index.lib.agentContext` exposes the parsed `sections`, the asserted `alwaysDoc`,
the `alwaysSections` / `progressiveSections` lists, and `mkProgressiveSkills`
(merge into `lib.skills.mkSkillsDir`'s `extraSkills`). Keep broad guidance in a
named fragment here; put one-off repository facts in that repository's own
fragment list.
