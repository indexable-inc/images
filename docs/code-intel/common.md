# Code intelligence

AST and semantic code analysis and rewriting for the `index` workspace. These
packages parse source into trees (mostly tree-sitter), match patterns or resolve
symbols against them, and turn the results into reports or byte-range edits. They
back the repo's own lint gate (`nix run .#lint` runs `astlog scan`, wired in
`lib/per-system.nix`), an AST-aware git merge driver, a clone/duplication
detector, a SCIP-backed find/replace, the Flecs query parser MCP server, and a
set of small reusable
crates (highlighting, identifier tokenizing, language detection, file walking,
edit application) shared with the [search](../search/common.md) and corpus
tooling.

Read this page first, then the component page for the package you are touching.

## Units

Every package is a Cargo workspace member of the root [`Cargo.toml`](../../Cargo.toml)
(`packages/...`) unless noted. A multi-crate package is one component directory;
its member crates are documented inside it.

| package | crates | role | flake output |
| --- | --- | --- | --- |
| [ast-merge](ast-merge/overview.md) | `ast`, `matcher`, `diff`, `langs`, `git`, `cli` | AST-aware git merge driver: GumTree tree matching + 3DM merge over tree-sitter | `ast-merge` |
| [astlog](astlog/overview.md) | `core`, `cli`, `py` | Datalog over tree-sitter ASTs: patterns as relations, joins as rules, rewrites as templates | `astlog` |
| [clone-detect](clone-detect/overview.md) | `hash`, `scanner`, `pragma`, `detect`, `cli` | Type-1/2/3 + sequence code clone detection | `clone` |
| [scipql](scipql/overview.md) | `core`, `cli`, `py` | lower a SCIP semantic index to Souffle facts, run datalog, apply find/replace | `scipql` |
| [flecs-query](flecs-query/overview.md) | `core`, `mcp`, `py` | parser for the Flecs Query Language: string to typed AST; MCP server | `ix-flecs-query-mcp` |
| [code-highlight](code-highlight/overview.md) | (single crate) | tree-sitter syntax highlighter rendering ANSI-colored source/snippets | none (library) |
| [code-tokenizer](code-tokenizer/overview.md) | (single crate) | tantivy tokenizer splitting identifiers on camel/snake/kebab boundaries | none (library) |
| [file-language](file-language/overview.md) | (single crate) | map a path/name/extension to its source language, no parser deps | none (library) |
| [repo-walker](repo-walker/overview.md) | (single crate) | iterator over text files honoring `.gitignore`, skipping binaries | none (library) |
| [edit-applier](edit-applier/overview.md) | (single crate) | apply sorted non-overlapping byte-range edits + render a unified diff | none (library) |
| [llm-clippy](llm-clippy/overview.md) | Nix-only | clippy fork tuned for LLM-assisted codebases | `llm-clippy` |

`ast-merge`, `astlog`, `clone-detect`, `scipql`, `flecs-query` are multi-crate
packages. The five small crates and `llm-clippy` build a single output each.
`llm-clippy` is the only Nix-only package here: it is excluded from the Rust
workspace and builds the [indexable-inc/clippy](https://github.com/indexable-inc/clippy)
fork from the `clippy-fork` flake input (`flake.nix:85`).

## How it fits together

Two crates inside the `ast-merge` package are the shared tree-sitter substrate
for this whole domain:

- `ast-merge-langs` ([langs](ast-merge/overview.md)) registers ~28 tree-sitter
  grammars and maps a path to a language profile. `astlog-core`,
  `clone-detect` (`hash`/`scanner`/`pragma`), and the `ast-merge` driver all
  resolve languages and grammars through it (root `Cargo.toml` dependents).
- `ast-merge-ast` ([ast](ast-merge/overview.md)) wraps tree-sitter parsing
  (`tree()`) and a recursive structural subtree hash (`compute`). `ast-merge`'s
  matcher/diff and all three `clone-detect` analysis crates parse and hash
  through it.

`astlog` does not use `ast-merge-ast`: it parses through `ast-merge-langs` and
runs tree-sitter's own `Query` engine directly (`packages/astlog/core/Cargo.toml`).
`file-language` is a parser-free language detector with the same curated variant
set as the highlighter; `code-highlight` depends on it
(`packages/code-highlight/Cargo.toml:14`) and layers the grammar-to-query
mapping on top.

The pattern across the active tools is **analyze -> query -> rewrite**:

```
source files
  -> parse / index        (tree-sitter via ast-merge-ast/langs; or rust-analyzer scip)
  -> relations / matches   (astlog Datalog rows; scipql Souffle facts; clone hashes; gumtree maps)
  -> findings              (astlog scan, clone JSON, query relations)   [report path]
  -> byte-range edits      (astlog rewrite templates; scipql edit relation)
       -> edit-applier::{apply, check_overlaps, unified_diff}           [rewrite path]
```

Both rewriters (`astlog`, `scipql`) converge on [edit-applier](edit-applier/overview.md)
for the apply/overlap/diff step (`packages/astlog/core/Cargo.toml`,
`packages/scipql/core/Cargo.toml`), so splice semantics cannot drift between
them.

## Invariants

- **Tree-sitter query strings are validated at load, not at match.** Both
  `astlog` (`Query::new`) and `code-highlight` (`HighlightConfiguration::new`)
  compile grammar queries up front; a misspelled node kind or field is a hard
  error with a position, never a silently empty result.
- **Rewrites are byte-range edits applied right-to-left.** An [`edit_applier::Edit`](edit-applier/overview.md)
  is `{file, start, end, replacement}`; edits in one file must be sorted and
  non-overlapping (`check_overlaps`), and `apply` splices them in reverse so
  earlier offsets stay valid. Overlap is a typed error, not a corrupt splice.
- **Identity vs text.** `astlog` matches on tree-sitter *syntax* (text/kind
  joins); `scipql` matches on resolved SCIP *monikers*, so `net::Socket` and
  `mock::Socket` are distinct symbols. Pick the tool by whether the question
  needs name resolution.
- **Dry run by default.** The rewriting CLIs (`astlog fix`, `scipql fix`,
  `scipql rename`) print a unified diff and only touch disk with `--write`.
- **Exit code is the lint contract.** `astlog scan` and `clone` exit non-zero
  when a blocking finding survives, so they gate CI directly.
- **Fall back, do not fail.** `ast-merge` drops to line-based merge on a parse
  error or structural conflict; `code-highlight` returns plain text for an
  unsupported language or a query failure. The user always gets output.

## Glossary

- **tree-sitter**: incremental parser generator; every grammar is a
  `tree-sitter-<lang>` crate. The shared parsing layer for this domain.
- **language profile** (`ast-merge-langs::Profile`): per-language metadata
  (extensions, file names, atomic/commutative/comment node kinds) driving merge
  and detection.
- **GumTree**: the tree-differencing algorithm `ast-merge-matcher` implements
  (top-down subtree hash matching, then bottom-up Dice-similarity matching) to
  map nodes between two revisions.
- **3DM / PCS**: the three-way structural merge model in `ast-merge-diff`. A
  `PcsTriple` is a (parent, predecessor, successor) ordering fact; conflicting
  triples from different sides are a structural conflict.
- **content hash vs normalized hash**: a clone-detection node carries an exact
  structural hash (Type-1) and a hash with identifiers renumbered and literals
  placeholdered (Type-2).
- **Type-1/2/3 clone**: identical (modulo whitespace), identical modulo
  identifier/literal renaming, and near-miss (structurally similar above a
  Jaccard threshold) duplicated code.
- **Datalog / fixpoint**: bottom-up rule evaluation to a fixed point.
  `astlog` runs a small built-in evaluator; `scipql` shells out to `souffle`.
- **moniker**: a SCIP symbol identity string (e.g.
  `rust-analyzer cargo mycrate 0.1.0 net/Socket#`). `scipql` keys facts and
  edits on it.
- **pragma** (`clone:ignore`): a source comment suppressing clone findings for
  a node, region, or file.
- **`astlog-ignore`**: a source comment suppressing `astlog scan` findings on
  its own line or the line below.

## Components

| component | page | what |
| --- | --- | --- |
| ast-merge | [ast-merge/overview.md](ast-merge/overview.md) | AST-aware git merge driver: GumTree matching + 3DM merge, line-based fallback |
| astlog | [astlog/overview.md](astlog/overview.md) | Datalog over tree-sitter ASTs: query/scan/fix CLI, lint gate, Python bindings |
| clone-detect | [clone-detect/overview.md](clone-detect/overview.md) | Type-1/2/3 + sequence clone detection, `clone.toml` config, SVG badge |
| scipql | [scipql/overview.md](scipql/overview.md) | SCIP index -> Souffle facts -> datalog query and find/replace/rename |
| flecs-query | [flecs-query/overview.md](flecs-query/overview.md) | Flecs Query Language parser to typed AST; stdio MCP server; Python bindings |
| code-highlight | [code-highlight/overview.md](code-highlight/overview.md) | tree-sitter ANSI highlighter for files and line-numbered snippets |
| code-tokenizer | [code-tokenizer/overview.md](code-tokenizer/overview.md) | tantivy identifier tokenizer (camel/snake/kebab splitting) |
| file-language | [file-language/overview.md](file-language/overview.md) | path/name/extension -> `Language`, no parser deps |
| repo-walker | [repo-walker/overview.md](repo-walker/overview.md) | gitignore-aware text-file iterator, binary-extension skip |
| edit-applier | [edit-applier/overview.md](edit-applier/overview.md) | apply sorted non-overlapping byte-range edits, render unified diff |
| llm-clippy | [llm-clippy/overview.md](llm-clippy/overview.md) | clippy fork with restriction lints for LLM-assisted codebases |
