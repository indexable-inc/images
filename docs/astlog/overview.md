# astlog

`packages/astlog` runs Datalog over tree-sitter syntax trees: a tree-sitter
query match becomes a relation (one row per match, one column per `@capture`),
Datalog rules join those relations (structurally, by value, or recursively),
`(rewrite ...)` forms turn derived rows into byte-range edits from templates, and
`(lint ...)` forms turn derived rows into located findings filtered by
`astlog-ignore` comments. It is the engine behind the repo's own lint gate:
`nix run .#lint` runs `astlog scan astlog-rules/nix.astlog` and the Rust rules
through a Nushell lint stage defined in `lib/per-system.nix`.

The full language reference, prior-art comparison, and design rationale are in
[`packages/astlog/README.md`](../../packages/astlog/README.md); this page is
the structural map.

## Member crates

| crate | id | role | flake output |
| --- | --- | --- | --- |
| `astlog/core` | `astlog-core` | the engine: reader, evaluator, rewrites, lints | none |
| `astlog/cli` | `astlog` | the `astlog` binary | `astlog` |
| `astlog/py` | `astlog-py` | PyO3 bindings (`import astlog` in the ix kernel) | none |

All three are Rust workspace members; only `cli` is a flake/packageSet output
(`packages/astlog/cli/default.nix`, `nix run .#astlog`). `core` depends on
[`ast-merge-langs`](../ast-merge/overview.md) for grammars and language
detection and on [`edit-applier`](../edit-applier/overview.md) for the rewrite
step (`packages/astlog/core/Cargo.toml`). It parses with tree-sitter directly
and matches with tree-sitter's `Query` engine; it does not use `ast-merge-ast`.

## The language (four S-expression forms)

- `(rule (head vars...) body-atom...)`: a relation. A `(match <lang> "<query>")`
  atom is a verbatim tree-sitter query; other atoms are rule names or builtins.
  Shared variables across body atoms are the join.
- `(rewrite <name> (relation vars...) (replace <target> "<template>"))`: replace
  the `<target>` node's bytes with the template, `{var}` splicing bound values.
- `(lint <relation> <severity> "<message template>")`: emit one finding per row,
  located at the row's first node-valued column.
- Rules may be recursive; every value is a node or text derived from one, so the
  universe is finite and naive fixpoint terminates.

Builtins, with arity (`core/src/program.rs:29-38`, `BUILTINS`): `ancestor/2`,
`parent/2`, `text/2`, `kind/2`, `same-text/2`, `same-file/2`, `text-match/2`
(regex, compiled once), `no-descendant/3`. No general negation or aggregates in
v0.

## core (`astlog-core`)

Module tree (`core/src/lib.rs:33-39`): `sexpr` (reader), `program` (forms +
validation), `corpus` (loaded files/nodes), `eval` (Datalog), `rewrite`
(edits/diff), `scan` (findings + suppression), `error`.

Entry point `analyze(rules, paths) -> Analysis` (`core/src/lib.rs:109-122`):
`Program::parse` -> `Corpus::load` (gitignore-aware via `ignore`) ->
`Evaluator::fixpoint` -> `rewrite::collect`. `Analysis`
(`core/src/lib.rs:57-100`) then offers `rewritten()` (changed files),
`diff()` (unified diff via `edit-applier`), `findings()` (sorted, suppressed),
and `suppressed()` (the audit view: each hidden finding plus the comment that
hid it). Key public types: `Program`/`Rule`/`Rewrite`/`Lint`/`Severity`
(`program.rs`), `Database`/`Relation`/`Row` (`eval.rs`),
`Corpus`/`Value`/`NodeRef` (`corpus.rs`), `Edit`/`FileRewrite` (`rewrite.rs`),
`Finding`/`SuppressedFinding` (`scan.rs`).

Tree-sitter queries are validated at load (`Query::new`): a misspelled node kind
or field is a hard error with a position, and `#`-predicates are rejected in
favor of builtins (README). Languages come from `ast-merge-langs`, detected by
extension, named by profile name or extension in `(match <lang> ...)`.

## cli (`astlog`)

Subcommands (`cli/src/main.rs:18-72`):

- `query RULES PATHS... [--relation R] [--json]`: print derived relations. Pure
  inspection, exits zero on success.
- `scan RULES [PATHS...] [--json] [--error]`: the lint gate. One finding per
  `(lint ...)` row minus `astlog-ignore` suppressions, sorted by
  (file, line, column, rule). Exits non-zero iff an error-severity finding
  survives; `--error` promotes warnings for the exit decision
  (`blocking_count`, `main.rs:197-205`). `--json` emits the contract array
  `{rule, severity, message, file, line, column, endLine, endColumn, text}`
  (`main.rs:222-243`).
- `suppressions RULES [PATHS...] [--json]`: list every suppressed finding with
  the comment behind it. Pure inspection.
- `fix RULES PATHS... [--write]`: print the rewrite diff, or apply with
  `--write` (`main.rs:170-186`).

`scan`/`suppressions` default `PATHS` to `.`; `query`/`fix` require paths.

## Suppression

A comment whose text contains `astlog-ignore` suppresses findings on its own
line (trailing) or the line below; `astlog-ignore: a, b` limits it to named
rules. Suppression filters at scan emission only, so the underlying Datalog rows
still exist for joins and `query` output (README "Suppression").

## py (`astlog-py`)

PyO3 `cdylib` (`py/src/lib.rs`), module `_astlog`, wrapped by
`python/astlog/__init__.py`. Conversion-only: `query`, `scan`, `suppressed`,
`fixes` return plain dicts/records keyed exactly like the `scan --json`
contract, and the Python wrapper builds polars frames; `fix(..., write=True)`
returns the unified diff (`py/src/lib.rs:56-205`). Not a flake output; built and
bundled into the ix-mcp kernel like the other `*-py` crates.

## In production

The repo's rules live in [`astlog-rules/nix.astlog`](../../astlog-rules/nix.astlog)
and [`astlog-rules/rust.astlog`](../../astlog-rules/rust.astlog), each lint
with a good/bad fixture pair under `astlog-rules/tests/` validated by the
`astlog-rules` flake check through the same `astlog scan --json` surface the gate
uses.
