# edit-applier

`packages/edit-applier` applies byte-range edits to source files and renders a
unified diff. A consumer derives a set of edits (a byte range plus replacement
text) from whatever analysis it runs, then hands them here to be checked for
overlap, applied, and diffed. The logic is corpus-agnostic on purpose:
[astlog](../astlog/overview.md) produces edits from tree-sitter rewrite
templates and [scipql](../scipql/overview.md) from Souffle `edit` relations over
a SCIP index, and both share this one apply/diff/overlap path so splice
semantics cannot drift (`src/lib.rs:1-13`). Single Rust workspace library crate
(`id = edit-applier`, no flake output); its only deps are `similar` (diffing) and
`snafu`.

## Public surface (`src/lib.rs`)

- `Edit { file, start, end, replacement }` (`lib.rs:26-32`): one replacement of
  `start..end` in file `file`, where `file` is an index into the caller's
  `files`/`paths` slice (the caller owns the file numbering). `Edit` is
  `Ord`, so callers sort with the derived ordering.
- `Source = (PathBuf, String)` (`lib.rs:22`): a file's path and current
  contents, the slice element `apply`/`unified_diff` consume.
- `FileRewrite { path, content }` (`lib.rs:35-39`): a file with all its edits
  applied.
- `check_overlaps(&[PathBuf], &[Edit]) -> Result<(), OverlapError>`
  (`lib.rs:65-88`): fail on the first pair of edits in the same file with
  overlapping ranges. Requires `edits` sorted ascending by `(file, start, end)`;
  it then only compares each edit to its successor. Adjacent ranges
  (`second.start == first.end`) do not overlap.
- `apply(&[Source], &[Edit]) -> Vec<FileRewrite>` (`lib.rs:97-119`): apply
  edits, returning only changed files. Within a file the edits are spliced
  right-to-left so earlier offsets stay valid as later ranges are replaced;
  non-overlap is the caller's responsibility.
- `unified_diff(&[Source], &[FileRewrite]) -> String` (`lib.rs:126-141`):
  `a/<path>` vs `b/<path>` unified diff of each rewrite against its original,
  via `similar::TextDiff`.
- `OverlapError` (`lib.rs:48-54`): the typed overlap error, naming the file and
  both ranges.

The ordering contract is the load-bearing invariant: sort, then `check_overlaps`,
then `apply`. `apply` itself re-sorts per file defensively but the overlap check
depends on the caller's sort. No flake package, no CLI: library only.
