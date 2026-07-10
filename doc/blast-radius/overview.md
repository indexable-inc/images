# blast-radius

`packages/blast-radius` reports how many `.#checks.x86_64-linux` derivations a PR
would rebuild and which changed inputs caused each rebuild. It is the engine
behind the sticky PR comment from `.github/workflows/blast-radius.yml`, and runs
locally as `nix run .#blast-radius` for the same report in Markdown.

## Purpose

A PR that touches a shared input can rebuild thousands of checks; one that edits a
single crate rebuilds a handful. blast-radius quantifies that radius and, crucially,
attributes it: it walks the derivation graph down to the *frontier* of change so
the report blames the crate source or dependency a human actually edited, not the
dozens of intermediate per-unit derivations whose hashes merely propagate the
change upward (`src/causes.rs:1-15`). This is meaningful only because every Rust
check is a per-unit [nix-cargo-unit](../nix-cargo-unit/overview.md) derivation
whose input-addressed `.drv` basename moves iff the build really changed.

## How it runs (`src/main.rs:196`)

```
blast-radius [BASE] [HEAD] [--json] [--timings PATH]
```

1. **Resolve revs** (`git::resolve`, `src/git.rs:36`): `BASE` defaults to
   `origin/main`, `HEAD` to `HEAD`; the diff base is their merge-base, so the
   report reflects only what this branch changed.
2. **Pick the catalog output** (`nix::catalog_attr`, `src/nix.rs:131`): prefer the
   sharded `ciChecks` flake output, but fall back to flat `checks` for both revs
   if either lacks it (a merge base can predate `ciChecks`); choosing per revision
   would mis-key every derivation as removed+added.
3. **Evaluate both revs concurrently** (`concurrent_evals`, `src/main.rs:123`):
   two `nix-eval-jobs` runs (pinned rev, eval cache off, 4 workers each so the
   peak memory matches one 8-worker eval), timing each thread's own work.
4. **Guard eval failures** (`guard_eval_failures`, `src/main.rs:68`): fail closed
   if any check newly fails to evaluate at head (a regression this change
   introduced); failures present at both base and head are pre-existing catalog
   issues, excluded from the diff and only reported.
5. **Diff** (`src/main.rs:216-247`): a check is `changed` when its attr's
   `drvPath` differs between base and head, `added`/`removed` by attr presence.
6. **Attribute causes** for the changed set (`compute_causes`, `src/main.rs:159`):
   load the recursive derivation graph at each rev with `nix derivation show
   --recursive` and walk to the frontier (below).
7. **Annotate timings** (optional `--timings`, `src/timings.rs`): per-attr
   wall-clock seconds from a prior successful Check run's nix-fast-build result
   file; a missing or unreadable file is ignored, not fatal (`src/main.rs:256`).
8. **Render**: `--json` emits `report.json` (the workflow contract); otherwise
   Markdown (`src/report.rs`).

## Root-cause frontier (`src/causes.rs`)

The derivation graph is keyed by `.drv` basename (`<hash>-<name>.drv`), which is
input-addressed: a head node is unchanged iff its basename also exists at base
(`is_changed`, `src/causes.rs:52`). `collect_frontier` (`src/causes.rs:71`) walks
the changed sub-DAG reachable from each rebuilt check and collects the *frontier*:
changed nodes whose own `.drv` inputs are all unchanged. Unchanged subtrees are
pruned and descent stops at each frontier node, so the traversal is bounded by the
change, not the closure. Only `.drv` inputs are followed, not bare source inputs:
a changed source already moves its consuming unit's basename, so a crate edit
lands on the readable `mynoise-0.1.0` unit rather than a raw `cargo-unit-source-*`
path (`src/causes.rs:64-70`). `root_causes` (`src/causes.rs:105`) ranks causes by
fan-out (how many checks each rebuilds) and applies the graph `Caps`
(`max_causes: 6`, `max_checks_per_cause: 5`, `src/main.rs:34`); the changed-check
list itself stays complete.

This replaces the old nushell tool's "blame every direct input whose hash moved",
which under per-unit builds credited every changed crate as a cause of every check
near it (a hairball); the frontier walk collapses that to the inputs actually
edited (`src/causes.rs:10-15`, tests at `src/causes.rs:173-320`).

## Report shape (`src/report.rs`)

`Report` (`src/report.rs:48`) serializes to `report.json`: `base`/`head` (short
SHAs), `total`, `changed`/`added`/`removed` attr lists, `categories` (counts by
the segment before the first dash, e.g. `rust`/`image`/`lint`,
`src/causes.rs:145`), `causes` (`{ name, checks }`), optional `timings`, and
`phaseTimings` (per-phase producer seconds, kebab-case stable keys). The JSON
schema is a contract with the trusted workflow job, which validates the shape and
rebuilds the comment; the Markdown renderer mirrors that job's renderer (capped
node graph, `CHANGED_LIST_CAP = 200` bullets) so a local run previews the comment
(`src/report.rs:16-23`).

## Attr normalization (`src/nix.rs:321`)

`nix-eval-jobs` joins nested attr paths with `.` and quotes any segment
containing a dot. A sharded `ciChecks` doctest leaf carries a file path
(`rust-foo."doctest-src/lib.rs - (line 12)"`), so `normalize_attr` drops every
`"` and splits on dots outside quotes, then rejoins, producing a bare name that
passes the workflow's safename regex and matches nix-fast-build's `--timings`
records identically (`src/nix.rs:302-334`).

## Build and packaging

`default.nix` selects the binary via `ix.cargoUnit.selectBinaryWithTests`
(MIT license). It is `inRustWorkspace`, `flake = true`, `packageSet = true`. Flake
output / main program: `blast-radius`. Deps: `clap`, `color-eyre`,
`serde`/`serde_json`. Modules: `git`, `nix`, `causes`, `report`, `timings`. An
end-to-end run needs a Linux builder (the per-unit Cargo graph is x86_64-linux,
import-from-derivation heavy; `src/nix.rs:1-7`).
