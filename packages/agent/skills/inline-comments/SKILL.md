---
name: inline-comments
description: "House rules for code comments: explain why a line exists, cite external constraints, delete narration. Use when writing or reviewing inline comments."
---

## Inline comments

Comments explain why a line exists, which failure it prevents, or which external
constraint pins the choice. They should add information the syntax cannot
recover. Delete comments that narrate the code.

Leave a comment when something looks redundant but a build, eval, or test proves
it is load-bearing. Put the observed symptom next to the line that survives the
obvious cleanup.

Non-obvious technical decisions need a public reference when one exists: RFC,
JEP, upstream issue, vendor doc, benchmark, errata, or design note. Put the URL
in the comment near the choice. If no public reference exists, say where the
decision came from.

Public helpers exposed through the flake `lib` output or `specialArgs.ix` use
per-binding `/** ... */` doc-comments. Document the argument shape, return
shape, and observable behavior. Keep implementation-only comments for the "why"
notes above.
