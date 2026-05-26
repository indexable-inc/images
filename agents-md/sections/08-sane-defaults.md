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

