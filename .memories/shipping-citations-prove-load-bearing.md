---
tldr: Grep the tree for citations of every file before deleting an archive; an author's own assessment cannot see its consumers
genre: memory
topic: [workflow]
handle: [_archive, rg-citations]
prior: 0.5
validated:
  - at: 2026-07-30T03:04:25Z
    by: claude-opus-4-6
    how: "rg -n 'antithesis-scratchbook' crates/ found 2 citations: cas_fabric_workload.rs:8 and cas/disk/src/tests.rs:2823"
    ok: true
---
`docs/_archive/antithesis-scratchbook/README.md` described its 50 files as "kept
for provenance only", written by the person who archived them one day earlier. It
was right about 48 of them.

The other two were cited from shipping Rust: `property-catalog.md` from
`crates/tools/antithesis-harness/src/cas_fabric_workload.rs:8`, and
`properties/presence-intent-co-commit.md` from
`crates/storage/cas/disk/src/tests.rs:2823` with a "keep this in sync with"
comment. The second turned out to be the spec for a live `assert_unreachable!`,
naming the assertion site down to the `Ok(None)` branch, plus the unit test whose
comment cites it back. A mutually load-bearing pair; deleting one half leaves the
other unexplained.

So before deleting any file in a triage, `rg` the whole tree for its path. A
citation from shipping code is evidence a file is load-bearing regardless of what
its author called it, because the author is describing intent and the citation is
describing use.
