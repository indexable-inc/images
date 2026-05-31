---
name: sane-defaults
disclosure: always
description: "Default to checked, typed, reproducible, root-cause work; reread the diff with suspicion before finishing."
---

## Sane defaults

Helpers, modules, packages, templates, examples, and generated commands should
be useful in the common production-shaped path without extra ceremony. Defaults
should be checked, typed, reproducible, conservative about secrets and
networking, and easy to override with a named reason.

Prefer the future-correct interface over compatibility layers. This repo can
change its own callers when an old spelling makes the safe path harder to
express. Remove migration branches and stale aliases in the same change that
introduces the clearer interface unless the user explicitly asks for a migration
window.

When the ecosystem already provides a robust tool for a large surface, push back
for at least one turn before rebuilding it here. Name the existing tool, the
maintenance cost, and the concrete gap that would justify ownership. If the gap
is real, track the work so the new surface keeps earning its weight.

For a small choice, lead with the direct answer and the shortest working path.
Save comparison tables for long-lived boundaries or vendor commitments.

Before finishing a change, reread the diff with suspicion. Ask whether the owner
is clear, whether a helper or type would remove real duplication, whether a
boundary is string-shaped when it should be typed, and whether a smaller API
would make the next change easier.

Fix root causes at the owner. When the same adapter, default, conversion, or
fallback appears in multiple places, move the capability transition or invariant
to the boundary that owns it.

Turn assumptions into checked behavior through types, schemas, module options,
derivation checks, or focused tests. If the user asked for a fix, land code and
the nearest durable test or validation hook; diagnosis alone is unfinished work.

Delete vestigial code in the same change that makes it obsolete. Dead fields,
options, structs, functions, configs, generated files, and compatibility shims
make the safe path harder to see.

When adding a non-obvious workaround, policy exception, or operational guard,
put the reason near the choice and cite a durable source when one exists.
