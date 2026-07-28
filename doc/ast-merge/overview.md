# ast-merge

`packages/ast-merge` is an AST-aware git merge driver: instead of merging text
lines it parses base/left/right with tree-sitter, matches nodes across the three
revisions with a GumTree-style tree differ, runs a 3DM structural merge, and
falls back to a clean line-based merge whenever parsing fails or a structural
conflict is found. It is one Cargo workspace package split into six member
crates (root `Cargo.toml:9-14`); the `cli` crate is the flake output
`ast-merge` (`packages/ast-merge/cli/default.nix`, `nix run .#ast-merge`). The
`ast` and `langs` crates are also the shared tree-sitter substrate for
[astlog](../astlog/overview.md) and [clone-detect](../clone-detect/overview.md).

## Member crates

| crate | id | role |
| --- | --- | --- |
| `ast-merge/ast` | `ast-merge-ast` | tree-sitter parsing + structural subtree hash |
| `ast-merge/matcher` | `ast-merge-matcher` | GumTree tree matching (node correspondence) |
| `ast-merge/diff` | `ast-merge-diff` | 3DM merge, PCS changesets, line-based fallback |
| `ast-merge/langs` | `ast-merge-langs` | language profiles + grammar registry + detection |
| `ast-merge/git` | `ast-merge-git` | git conflict marker parsing/formatting, revision IO |
| `ast-merge/cli` | `ast-merge` | the `ast-merge` binary (flake output) |

All six are Rust workspace members (`inRustWorkspace = true` in each
`package.nix`); only `cli` is also a flake/packageSet output.

## CLI surface (`cli/src/main.rs`)

`ast-merge <command>` with four subcommands (`main.rs:18-42`):

- `merge BASE LEFT RIGHT [-o OUT] [-l LANG] [--git]`: three-way merge. Without a
  detected/forced language it line-merges; `--git` writes the result back to
  `LEFT` (the merge-driver convention), otherwise to `-o`/`OUT` or `LEFT`
  (`merge.rs:45-51`). Exit code `1` (`EXIT_CONFLICTS`, `main.rs:8`) signals
  conflicts to git.
- `solve FILE [-o OUT]`: parse existing conflict markers in a file (conflict
  resolution is not yet fully implemented; logs a warning, `main.rs:130-156`).
- `languages`: list supported languages with extensions/file names.
- `info`: print version and the git merge-driver setup snippet.

Wiring it as a git driver (`info` output, `main.rs:191-202`):

```
# .gitattributes
*.rs merge=ast-merge
# git config
git config merge.ast-merge.driver 'ast-merge merge %O %A %B --git'
```

Tracing goes to stderr via `RUST_LOG` (default `info`, `main.rs:84-92`).

## Pipeline (`cli/src/merge.rs:105-138`)

```
resolve_language(--language | path)            langs::detect / detect_by_name
parse base/left/right                          ast::tree(src, &grammar)
  any parse error -> diff::based (line merge)   merge.rs:112-115
matcher::compute(base, left)                   GumTree Map base<->left
matcher::compute(base, right)                  GumTree Map base<->right
diff::ThreeWay { trees, both matchings }.merge()
  structural conflict -> diff::based (line merge with git markers)
```

### ast (`ast-merge-ast`)

Public surface (`ast/src/lib.rs:5-7`): `tree(source, &Language) -> Output`
(parse, exposing `Tree`, `has_errors`, `PreorderIterator`), `compute(&Tree,
node) -> u64` (structural subtree hash), and `Node`/`NodeId`/`Revision`. The
hash (`ast/src/types.rs:80-97`) folds node kind plus child hashes (or leaf byte
span) with `FxHasher`, so two subtrees hash equal iff structurally identical.
`Revision` is the `Base`/`Left`/`Right` tag used throughout the merge.

### matcher (`ast-merge-matcher`)

`compute(tree_a, tree_b) -> Map` (`matcher/src/lib.rs:13-23`) runs GumTree
(`gumtree.rs`): a top-down phase hashes nodes above `min_height` and matches
unique subtree-hash pairs (`gumtree.rs:98-162`), a leaf/sibling phase matches
small unmatched subtrees, and a bottom-up phase (`gumtree::bottomup`) matches
parents by Dice coefficient over already-matched descendants. `Config`
(`config.rs`) defaults: `min_height = 2`, `dice_threshold = 0.5`,
`max_ted_size = 100`. `rayon` parallelizes matching. The result `Map` is the set
of `(NodeId, NodeId)` correspondences the diff consumes.

### diff (`ast-merge-diff`)

`ThreeWay::new(ThreeWayParams { trees, base_left_matching, base_right_matching,
config }).merge() -> Result` (`diff/src/engine.rs:59-89`). It builds a `Class`
equivalence mapping from the two matchings, extracts `PcsTriple`s (parent,
predecessor, successor) per revision into a `ChangeSet` (`changeset.rs`), then
`detect_conflicts` applies the 3DM inconsistency rule: triples sharing a
(parent, predecessor) but with different successors, coming from different
non-base sides, and not converging to the same class, are a structural conflict
(`engine.rs:147-276`). No conflict -> `reconcile_lists` reconstructs merged
top-level items; conflict -> fall back to `based()` (`lines.rs`) which emits
git-style markers with full text context. `Config` defaults: `marker_size = 7`,
`diff3_style = true` (`engine.rs:28-43`). Public types: `ChangeSet`,
`PcsTriple`, `Conflict`, `Region`, `Result`, `ThreeWay*`, `Config`, `based`,
`Class` (`diff/src/lib.rs:10-14`).

### langs (`ast-merge-langs`)

`Lang` enum + `detect(path) -> Option<Lang>`, `detect_from_extension`,
`Profile` (`langs/src/lib.rs:5-6`). A `Profile` (`langs/src/types.rs`) carries
`name`, `extensions`, `file_names`, plus the merge-relevant `atomic_nodes`,
`commutative_parents`, and `comment_nodes` predicate sets. The crate depends on
~28 `tree-sitter-<lang>` grammar crates (`langs/Cargo.toml`: Rust, JS/TS,
Python, Go, JVM langs, C/C++, C#, Swift, Ruby/PHP/Bash/Lua, Haskell/Elixir/
OCaml, HTML/CSS/Svelte, JSON/TOML/YAML, Markdown, Dockerfile, Nix) and is the
grammar registry the rest of this domain reuses. `Lang::to_tree_sitter()` yields
the grammar for parsing.

### git (`ast-merge-git`)

Revision IO and git conflict-marker handling (`git/src/lib.rs:5-7`):
`read_revision`/`write_result` (file IO returning `RevisionError`),
`conflicts(content) -> ParsedFile` (parse `<<<<<<<`/`=======`/`>>>>>>>`
markers), `conflict`/`extract_oid_from_marker` formatting, and
`DisplaySettings`/`DriverResult`/`ParsedConflict`/`ParsedFile` types. Depends
only on `snafu` and `lazy-regex`, no tree-sitter.

## Build and tests

Every member crate sets `passthruTests = true`, so each is checked through the
cargo-unit Rust policy (`packages/*/package.nix`). `cli/default.nix` selects the
`ast-merge` binary with `ix.cargoUnit.selectBinaryWithTests`. There is no extra
runtime wrapper: the grammars are statically linked, so `nix run .#ast-merge`
needs nothing on `PATH`.
