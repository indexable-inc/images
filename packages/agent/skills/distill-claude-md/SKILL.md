---
name: distill-claude-md
description: >-
  Compress and deduplicate an instruction document (CLAUDE.md, AGENTS.md, a skill, a memory, a
  system prompt, any rules file) without losing behavior. Use when the user wants to tighten,
  distill, dedupe, generalize, shrink, clean up, or say something more concisely for any
  instruction or rules doc, or when one has grown sprawling and repetitive. The goal is the
  smallest text that still produces the same behavior, by generalizing specifics into the rule
  they illustrate, factoring shared conditions, merging duplicates to one source of truth,
  cutting completeness theater, and keeping every load-bearing handle (trigger, action, the
  non-obvious why, and the concrete command, path, or flag).
---

# Distilling an instruction doc

Maximize behavior per token. The target is the *smallest* text that still produces the *same* behavior. As simple as possible, not simpler: every cut must preserve what the rule actually changes about what the agent does.

A rule is load-bearing only in three parts. Compression may rewrite everything else; it must keep all three:

- **Trigger** — when the rule fires (the condition).
- **Action** — what to do (or not do) when it fires.
- **The non-obvious why / the handle** — the gotcha, constraint, or source that makes the rule non-trivial, plus any concrete command, path, flag, or number needed to act. This is the part that cost someone real time to learn. Losing it makes a future agent relearn it the hard way.

Everything else (preamble, restated context, hedging, a second example that adds no new case, "happily", balanced "X not Y" cadence) is theater. Cut it.

## The five moves

Run these over the whole doc, not line by line.

1. **Deduplicate to one source of truth.** The same fact stated in two sections drifts and contradicts. Pick the best statement, delete the rest, link if a cross-reference is genuinely needed. Watch for near-duplicates phrased differently: they are the same rule and must collapse to one.

2. **Lift specifics into the general rule they illustrate.** A rule written as a single example is usually an instance of a broader invariant. State the invariant, demote the example to *at most* one short illustration (or cut it if the rule is clear without it). This is the AGENTS.md principle: extract the reusable rule, do not enshrine the example as policy.

3. **Factor shared structure (algebraic).** When N rules share a condition or an action, factor it: `ab + ac = a(b+c)`. State the common part once, then list only the deltas. A cluster of "in repo X do Y", "in repo X do Z" becomes "in repo X: Y; Z."

4. **Move narrow facts to their owner.** A doc loaded every turn should hold only cross-cutting rules. A fact that matters to one file, command, or subsystem belongs next to that thing (a code comment, a memory, a topic skill, the narrow owner), not in the always-on prose. Relocating shrinks the hot doc and puts the fact where it is found in context.

5. **Cut completeness theater.** Remove anything that restates context, narrates intent, hedges, or pads cadence. State the desired thing directly and stop.

## Do not over-compress

The failure mode is dropping a hard-won specific to save a line. Guards:

- **The relearn test.** Before deleting a sentence, ask: would a future agent waste time rediscovering this, or do the wrong thing without it? If yes, it is load-bearing. Keep it (possibly relocated), do not cut it.
- **Do not merge genuinely different rules** behind a forced abstraction. Two rules that look similar but fire on different triggers or want different actions stay separate. Three precise lines beat one vague umbrella. (Same instinct as preferring three similar lines over a premature shared helper.)
- **Keep the concrete handle.** Never abstract away the command, path, flag, URL, or number. "Validate with nix" is worse than "validate with `nix build .#checks.<system>.<name>`." Specifics are the point, not the fat.
- **Preserve absolutes.** A rule stated as absolute ("never", "always", "this is not a preference") must stay absolute after rewriting. Do not soften it into a suggestion while trimming words.

## Procedure

1. **Read the whole doc first.** You cannot dedupe or factor what you have not seen end to end. Build a mental (or scratch) index of every distinct claim.
2. **Cluster.** Group claims that are duplicates, near-duplicates, or share a condition/action. Each cluster collapses to one statement.
3. **Rewrite each cluster** to its smallest form using the five moves, preserving trigger + action + why/handle.
4. **Relocate** narrow facts to their owner; leave only cross-cutting rules in the hot doc.
5. **Diff for behavior, not just length.** Reread the new doc against the old asking only: does any trigger, action, absolute, or handle present in the old version no longer reach the agent? If so, you cut something load-bearing. Restore it.
6. **Apply and validate** (below). A shorter doc that changed behavior is a regression, not a win.

## Apply and validate

- **Edit the source, not a symlink.** The global `CLAUDE.md` source is `~/.config/nix/claude/global/CLAUDE.md`; the `~/.claude/CLAUDE.md` path is a home-manager symlink into the nix store. Global skills live under `~/.config/nix/claude/global/skills/<name>/SKILL.md`. Both go live only after `home-manager switch --flake ~/.config/nix#andrewgazelka@hydra`. A project `CLAUDE.md` is live immediately.
- **Prove behavior survived.** Compression's whole risk is silently changing behavior. After applying, use the `prompt-eval` skill: spawn fresh `claude -p` sessions on neutral tasks that should trigger the rules you rewrote, and confirm the behavior still emerges. If a rule no longer fires, you over-compressed it. This is the only real proof; reading the diff is not enough.

## Output

Report the distilled doc, the byte/line delta, and a short list of what moved where (merged clusters, lifted generalizations, relocated facts). Flag explicitly anything you were tempted to cut but kept as load-bearing, so the user can overrule.
