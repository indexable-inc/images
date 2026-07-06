# clone-detect

`packages/clone-detect` finds duplicated code across a tree. It parses every
recognized file with tree-sitter, extracts significant subtrees, hashes them
two ways (exact and identifier/literal-normalized), and reports clone groups:
Type-1 (identical), Type-2 (identical modulo renaming), Type-3 (near-miss,
structurally similar above a threshold), and statement Sequences. The `clone`
binary emits JSON and a duplication percentage and gates on **budgets**: it
exits non-zero when an enabled gate's metric exceeds its ceiling (see
[Gates](#gates)), so it can gate CI without failing on every pre-existing clone.
A repo-level [`clone.toml`](../../clone.toml) tunes thresholds, ignore globs, and
the `[budget]` ceilings (`nix run .#clone`).

## Member crates

| crate | id | role |
| --- | --- | --- |
| `clone-detect/hash` | `clone-hash` | per-node content + normalized hashing, significant-node extraction |
| `clone-detect/pragma` | `clone-pragma` | `clone:ignore` comment pragmas |
| `clone-detect/scanner` | `clone-scanner` | parallel tree walk -> files + hash index |
| `clone-detect/detect` | `clone-detect` | the detection algorithms (Type-1/2/3, sequences) |
| `clone-detect/cli` | `clone` (pkg `clone-cli`) | the `clone` binary (flake output) |

All five are Rust workspace members; only `cli` is the flake/packageSet output
(`packages/code/clone-detect/cli/default.nix`, binary `clone`, package `clone-cli`,
`nix run .#clone`). `hash`, `scanner`, and `pragma` build on
[`ast-merge-ast`](../ast-merge/overview.md) (parse + structural hash) and
[`ast-merge-langs`](../ast-merge/overview.md) (grammars + detection).

## Pipeline (`cli/src/main.rs:157-199`)

```
find_config(path)            walk up for clone.toml (cli/src/main.rs)
resolve_config              CLI flags > clone.toml > defaults (main.rs)
Scanner::directory(path)    parse + index every file (scanner)
instances(scan, config)     Type-1/2/3 + sequences (detect)
filter::by_patterns         drop --ignore / clone.toml globs (cli/src/filter.rs)
evaluate_gates              global + (optional) diff budgets (cli/src/gate.rs)
output_json                 DetectionResult + gate object as JSON (pretty optional)
badge::write (optional)     SVG duplication badge (cli/src/badge.rs)
```

`run` returns "any enabled gate failed", which `main` maps to exit
`FAILURE`/`SUCCESS`.

## hash (`clone-hash`)

`significant_nodes(&tree, min_lines, min_nodes) -> Vec<NodeInfo>` selects
subtrees large enough to be clone candidates; each `NodeInfo` carries kind, byte
range, line range, node count, `subtree_features`, and the two hashes
(`hash/src/lib.rs:6-8`, `extract.rs`). `compute` (re-exported from
`ast-merge-ast`) is the exact structural hash (Type-1 key). `hash` (the
normalized hash, `normalize.rs`) walks the subtree renumbering identifiers in
order of first appearance and replacing every literal with a fixed placeholder
(`LITERAL_PLACEHOLDER`, `normalize.rs:8`), so two fragments that differ only in
names/literals hash equal (Type-2 key). `kinds::is_significant`/`is_identifier`/
`is_normalizable` decide what counts.

## pragma (`clone-pragma`)

`scan(&tree) -> Info` walks comments for `clone:ignore` pragmas
(`pragma/src/lib.rs:84-148`): `clone:ignore` (next node), `clone:ignore-start`/
`clone:ignore-end` (a region), `clone:ignore-file` (whole file). The space after
the colon is rejected, so pragmas must be tight. `Info::is_ignored(range)` is
how the scanner drops ignored nodes.

## scanner (`clone-scanner`)

`Scanner::new(Config).directory(path) -> Output` (`scanner/src/scan.rs:95-162`)
walks with `ignore::WalkBuilder` honoring gitignore (always skipping `.git`),
capped at `MAX_THREADS = 8` parallel parses, detecting language per file,
skipping symlinks/non-UTF-8/`ignore-file` files, and collecting
`significant_nodes` minus pragma-ignored ranges. `Config` defaults `min_lines =
5`, `min_nodes = 10` (`scan.rs:35-49`). The `Output` holds the `files` and a
`Hash` index (`index.rs`) mapping `content_hash -> locations` and
`normalized_hash -> locations`, exposing `type1_candidates()` /
`type2_candidates()` (buckets with more than one location).

## detect (`clone-detect`)

`instances(&Output, &DetectConfig) -> DetectionResult` (`detect/src/detector.rs:16-120`):

- **Type-1**: content-hash buckets with 2+ fragments (`detector.rs:20-36`).
- **Type-2**: normalized-hash buckets with 2+ fragments, skipping any already
  fully covered by Type-1 (`detector.rs:38-62`).
- **Type-3** (`--type3`, `detect/src/type3.rs`): per node-kind, dedup by
  normalized hash, build a MinHash signature over `subtree_features`, bucket
  with LSH (`lsh.rs`), pre-filter candidate pairs by estimated Jaccard, then
  confirm with multiset Jaccard against `type3_threshold` (default `0.7`).
  `rayon`-parallel across kinds.
- **Sequence** (`--sequences`, `sequences.rs`): sliding window of statements.
- `dedup_subsumed` drops groups whose fragments are byte-range contained in a
  larger group (e.g. a function and its block, `detector.rs:139-202`).

`DetectionResult` (`types.rs:65-84`) is `{ instances: Vec<CloneGroup>, stats }`;
`Kind` is `Type1 | Type2 | Type3 { similarity } | Sequence { statements }`;
`stats.duplication_pct` is deduplicated duplicated lines over total lines. All
types are `serde`-serializable (the JSON output schema).

## cli (`clone`) config and flags

Flags (`cli/src/main.rs`): `[PATH]` (default `.`), `--type3`, `--threshold`,
`--min-lines`, `--min-nodes`, `--sequences`, `--window-size`, `--ignore`
(repeatable glob), `--pretty`, `--badge PATH`, plus the gate flags
`--max-global-pct`, `--max-diff-pct`, and `--diff [BASE]` (see [Gates](#gates)).
`clone.toml` mirrors these keys (`FileConfig`) and adds a `[budget]` table; CLI
flags win, booleans OR, ignore lists concatenate (`resolve_config`). The repo's
[`clone.toml`](../../clone.toml) sets `min_lines = 8`, `min_nodes = 15`, ignore
globs for vendored/generated/test trees, and `[budget] global_pct` as the
duplication ratchet.

## Gates

Two independent budget gates decide the exit code (`cli/src/gate.rs`). Each is
"metric must be `<=` its ceiling"; a metric exactly equal to the ceiling passes.
The run exits `FAILURE` iff any enabled gate fails (or on error).

- **global** (always evaluated): `stats.duplication_pct` over the whole scan vs
  `global_pct`. Its ceiling is `--max-global-pct` > `[budget] global_pct` >
  `0.0`. The `0.0` default reproduces the legacy behavior (any surviving clone
  fails), so a repo with no `[budget]` and no gate flags gates exactly as before.
- **diff** (opt-in, `--diff`): duplication concentrated on lines changed since a
  git base rev. Changed lines are the added/modified new-side lines of
  `git diff <merge-base(base, HEAD)>` taken against the working tree, so
  uncommitted edits count (`cli/src/diff.rs`). A changed line is "duplicated"
  when a surviving clone fragment in that file covers it;
  `diff_pct = 100 * duplicated / changed` (`0` when nothing changed). The base is
  the `--diff` argument > `[budget] diff_base` > `origin/main`; the ceiling is
  `--max-diff-pct` > `[budget] diff_pct` > `0.0`. If git is missing or the base
  rev is unknown, the run fails loudly rather than skipping the gate.

The diff gate shells out to `git` with user/system config and external diff
drivers disabled, so a custom `~/.gitconfig` pager or `diff.external` cannot
corrupt the unified-diff parse. It never runs in the CI lint derivation (that
copy has no `.git`); CI gates only on `global`.

### `[budget]` config

```toml
[budget]
global_pct = 1.1        # whole-scan duplication_pct ceiling
diff_pct = 0.0          # changed-line duplication ceiling (used with --diff)
diff_base = "origin/main"
```

### `gate` JSON

`output_json` merges a `gate` object into the existing `{ instances, stats }`
schema. A gate key is present only when the gate is enabled:

```json
{
  "instances": [ ... ],
  "stats": { ... },
  "gate": {
    "global": { "duplication_pct": 1.05, "budget_pct": 1.1, "pass": true },
    "diff": {
      "diff_pct": 0.0,
      "budget_pct": 0.0,
      "pass": true,
      "base": "origin/main",
      "base_sha": "aa24dc0b...",
      "changed_lines": 12,
      "duplicated_changed_lines": 0
    }
  }
}
```

Each gate also logs a one-line PASS/FAIL summary to stderr via `tracing`.
