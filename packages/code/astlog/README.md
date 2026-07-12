<p align="center"><img src="assets/hero.svg" width="720" alt="source files become relations via tree-sitter query matches, Datalog rules join them into derived rows, and derived rows become lint findings or rewrite edits"></p>

# astlog

How do you grep for an `unwrap()` call *inside a function whose return type is `Result`*, or a call passing a variable *that was assigned from `getenv`*? Pattern tools (ast-grep, Semgrep, tree-sitter queries) answer "does this node match this shape"; those questions are joins, the first on tree position, the second on identifier text across two unrelated subtrees. astlog is Datalog over tree-sitter syntax trees: query matches become relations, rules join them, and rewrites turn derived rows into edits. Both questions are one rule.

## Get it

```sh
nix run github:indexable-inc/index#astlog -- --help
```

With cargo (the CLI crate is `astlog`):

```sh
cargo install --git https://github.com/indexable-inc/index astlog
```

The Python surface ships inside the ix kernel (`import astlog`). Source lives
in the monorepo: `git clone https://github.com/indexable-inc/index`.

## The language

A rules file is S-expressions, four forms:

```lisp
;; tree-sitter query matches become relations: one row per match,
;; one column per @capture
(rule (unwrap-call call e)
  (match rust "
    (call_expression
      function: (field_expression value: (_) @e field: (field_identifier) @m)
      arguments: (arguments)) @call")
  (text m "unwrap"))

(rule (result-fn f)
  (match rust "
    (function_item return_type: (generic_type type: (type_identifier) @r)) @f")
  (text r "Result"))

;; the join: shared variables across body atoms, exactly like columns
;; shared across SQL tables; `ancestor` is a structural builtin
(rule (fixable call e)
  (unwrap-call call e)
  (result-fn f)
  (ancestor f call))

;; derived rows become edits: replace the target node with a template
;; splicing bound variables
(rewrite unwrap-to-try (fixable call e)
  (replace call "{e}?"))

;; derived rows become findings: `astlog scan` reports every `fixable` row
;; as an error located at the row's first node-valued column, rendering the
;; message template against the relation's head variables
(lint fixable error "unwrap inside a Result-returning function: `{e}`")
```

Rules may be recursive (`(rule (up x z) (up y z) (parent x y))`); every value
is a syntax node or text derived from one, so the universe is finite and
naive fixpoint iteration terminates. Builtins: `ancestor`, `parent` (tree
position), `text`, `kind`, `same-text` (values), `same-file`,
`(text-match <node-or-text> "<regex>")` (regex over the value's text; the
pattern must be a string literal and is compiled once at setup), and
`(no-descendant <node> "<kind>" "<text>")` (holds when the node has no strict
descendant with that kind and exact source text, the narrowest absence check
the lint rules need). General negation and aggregates are deliberately absent
in v0.

Because patterns are verbatim tree-sitter queries, `Query::new` validates
node kinds and field names against the grammar at load: a misspelled node
kind is a hard error with a position, never a silently empty result. `#`
predicates (`#eq?` etc.) are rejected with guidance to use builtins, which
the Datalog layer subsumes.

## Surfaces

- **Library**: `astlog-core` (this directory's `core/`), the only place with
  logic.
- **CLI**:
  - `astlog query rules.astlog src/ [--relation r] [--json]` prints derived
    relations (pure inspection, always exits zero on success).
  - `astlog scan rules.astlog [paths...] [--json] [--error]` is the lint
    gate: one finding per row of each `(lint ...)`-declared relation, located
    at the row's first node-valued column, sorted by (file, line, column,
    rule). Exit code is nonzero iff an error-severity finding survives
    suppression; `--error` promotes warnings for the exit decision (parity
    with `ast-grep scan --error`). `--json` emits an array of
    `{"rule","severity","message","file","line","column","endLine",
    "endColumn","text"}` objects; this shape is a contract for downstream
    consumers. Adding a `(lint ...)` form extends the gate without touching
    the invocation (this is how `nix run .#lint` runs
    `astlog-rules/nix.astlog`).
  - `astlog suppressions rules.astlog [paths...] [--json]` lists every finding
    an `astlog-ignore` comment suppresses, each with the comment that hid it
    (its line and text), so an audit can answer "what are we explicitly
    ignoring, where, and why". Pure inspection: always exits zero on success.
    `--json` emits the `scan` shape plus `"commentLine"` and `"commentText"`.
  - `astlog fix rules.astlog src/ [--write]` applies rewrites.
- **Python**: `import astlog` in the ix kernel (bundled like `search`/`tui`).
  Results come back as polars DataFrames like every other bundled module:
  `astlog.query(rules, paths)` returns `dict[str, pl.DataFrame]`, one frame per
  relation, every column a struct of the seven node fields (a derived text
  value fills only `text`); `astlog.scan(...)`, `astlog.fixes(...)`, and
  `astlog.suppressed(...)` each return a `pl.DataFrame`; `astlog.fix(...,
  write=True)` returns the unified diff. The Rust bindings are
  conversion-only: they hand back records and the Python wrapper builds the
  frames.

## Suppression

A comment whose text contains `astlog-ignore` suppresses findings located on
the comment's own line (trailing comment) or the line immediately below it.
`astlog-ignore: name1, name2` limits suppression to those rule names; a bare
`astlog-ignore` suppresses every rule there. Comment nodes are any
tree-sitter node whose kind contains "comment" (nix `comment`, rust
`line_comment`/`block_comment`, ...). Suppression filters findings at scan
emission only: the underlying Datalog rows still exist for joins and `query`
output. `astlog suppressions` (and `astlog.suppressed` in Python) lists the
findings that filter removed, each paired with the comment that suppressed it,
so a suppression backlog stays auditable rather than invisible.

Languages come from `ast-merge-langs`: every grammar that crate registers
(Rust, Python, TypeScript, Go, Nix, ...) works here, detected by file
extension, named in `(match <lang> ...)` by profile name or extension.

## Prior art

This composes ideas that each exist somewhere, but not together:

| Project | What it has | What it lacks for this use |
| --- | --- | --- |
| [treeedb](https://github.com/langston-barrett/treeedb) | tree-sitter ASTs as Soufflé Datalog relations | selection only, no rewriting; Soufflé toolchain; unmaintained since ~2022 |
| [Logifix](https://github.com/lyxell/logifix) | Datalog-driven rewriting (the closest in spirit) | Java only; rules compiled with Soufflé, not loaded at runtime |
| [ql-grep](https://github.com/travitch/ql-grep) | CodeQL syntax over tree-sitter | selection only, no rewriting |
| [CodeQL](https://codeql.github.com) | the relational model done at depth (real dataflow) | heavyweight extraction per language; not embeddable; no rewriting |
| [ast-grep](https://ast-grep.github.io) | patterns-as-code + rewrites, `inside`/`has` constraints | constraints are structural-only: no value joins, no recursion, no cross-pattern joins |
| [GritQL](https://github.com/biomejs/gritql) | pattern language with `where` clauses + rewrites | same: no general relational layer |
| [Semgrep](https://semgrep.dev) / [Coccinelle](https://coccinelle.gitlabpages.inria.fr/website/) | metavariable patterns; Coccinelle's diff-style patches | per-language engines; constraint layer is ad hoc, not Datalog |
| [weggli](https://github.com/weggli-rs/weggli) | C/C++ sketches over tree-sitter | C/C++ only, selection only |
| [Glean](https://glean.software) (Angle) | code facts queried relationally at scale | an indexing service, not an embeddable rewriter |

The gap astlog fills: **tree-sitter patterns as relations + runtime Datalog
joins (positional, by value, recursive) + rewriting, embeddable as a library
with CLI and Python bindings.**

## Built from scratch vs reused

Deliberately reused, because rebuilding any of these is the classic trap:

- **Parsing and grammars**: tree-sitter plus the ~28 grammar crates already
  in this workspace via `ast-merge-langs`, including language detection.
- **Matching**: tree-sitter's own `Query` engine is the pattern matcher;
  astlog never walks trees to match shapes. Grammar-validated queries
  (typo = load error) come free with it.
- **Plumbing**: `ignore` (gitignore-aware walking), `similar` (diffs),
  `clap`, `snafu`, pyo3, and the repo's cargo-unit/Nix/kernel-bundling
  infrastructure.

Deliberately written here, after evaluating the alternatives:

- **The Datalog evaluator** (~300 lines, naive fixpoint, nested-loop joins).
  `ascent`/`crepe` fix rules at Rust compile time (ours load from a file at
  runtime); `datafrog` wants statically-typed tuple relations and
  hand-written join plans, so the dynamic binding layer would have to be
  written anyway; Soufflé is an external C++ codegen toolchain, the wrong
  shape for an embedded library; `cozo` would work but imposes CozoScript as
  the user language plus a database's worth of dependency for features
  (persistence, vectors) this never uses. At single-repo scale the engine is
  trivial; the DSL boundary is the part worth owning. If rule sets outgrow
  naive evaluation, swapping datafrog or cozo underneath does not change the
  language.
- **The relation boundary and builtins**: captures → rows, `ancestor`/
  `parent`/`text`/`kind`/`same-text`/`same-file`, template splicing. This
  glue is the product; nothing ships it.
- **The S-expr reader** (~150 lines): `lexpr` exists, but per-form line
  numbers drive every diagnostic here and the reader is trivial.

## v0 limitations (deliberate)

- Naive (not semi-naive) fixpoint; fine at repo scale, measured before
  optimized.
- No negation, no aggregates, no stratification to get wrong.
- A quantified capture (`+`/`*`) contributes its first node per match.
- `ancestor`/`parent` need their lower argument bound by atoms to their left
  (left-to-right evaluation), and report exactly that when violated.

## In production

The repo's lint rules live in `astlog-rules/nix.astlog` (Nix) and
`astlog-rules/rust.astlog` (the corpus/search Rust crates), both ported from
the old ast-grep YAML rules (#1060 did the Nix rules, `prefer-sri-hash`
followed in #1062 once suppression comments landed) and gating `nix run .#lint`
via `astlog scan`. Each lint has a committed good/bad fixture pair under
`astlog-rules/tests/`, validated by the `astlog-rules` flake check through the
same `astlog scan --json` surface the gate uses.

## Planned

- A concrete-syntax pattern front end (`$X.unwrap()` style, likely via
  `ast-grep-core`) compiling into the same relations, so simple rules need no
  tree-sitter query vocabulary.
