---
tldr: Repointing a code comment at a replacement document is only correct if that document contains the named thing
genre: memory
topic: [workflow]
handle: [supersedes, dangling-citation]
prior: 0.5
validated:
  - at: 2026-07-30T03:04:25Z
    by: claude-opus-4-6
    how: "rg presence-intent-co-commit antithesis/redesign.md in the ix repo: 0 hits"
    ok: true
---
When deleting a documentation file that shipping code cites, the obvious fix is
to repoint the comment at whatever superseded it. That is wrong whenever the
replacement restates the material in its own words instead of carrying the names
forward.

Measured instance, 2026-07-29: an agent checked all 39 property slugs from
`docs/_archive/antithesis-scratchbook/properties/` against `antithesis/redesign.md`
and found **0 of 39** appear by name. It then deleted the files and repointed two
Rust comments at `redesign.md` anyway. A reader following
`crates/storage/cas/disk/src/tests.rs:2823` would grep for
`presence-intent-co-commit`, find nothing, and be unable to tell whether the test
had drifted or the doc had.

A citation that looks correct and is not costs more than a dangling path, because
a dangling path announces itself. So: grep the replacement for the specific name
before repointing. If it is absent, keep the source (convert it) rather than
redirecting to a document that does not answer the question.

The wider lesson from the same incident, which is the more useful one: having the
evidence and not applying it is worse than not having it. The 0-of-39 check was
already in hand.
