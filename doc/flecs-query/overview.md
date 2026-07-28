# flecs-query

`packages/flecs-query` is a pure-Rust parser for the
[Flecs Query Language](https://github.com/SanderMertens/flecs/blob/master/docs/FlecsQueryLanguage.md),
the string format flecs uses for ECS queries
(`Position, [in] Velocity, (ChildOf, $parent)`). Flecs ships no standalone
grammar: upstream's language is a hand-written C parser bound to a live
`ecs_world_t`. This crate reimplements the same grammar as a standalone,
world-independent parser, exposes it as a typed AST, an MCP server, and Python
bindings, and round-trips (`parse(q.to_string()) == q`).

## Member crates

| crate | id | role | flake output |
| --- | --- | --- | --- |
| `flecs-query/core` | `flecs-query-core` | the parser: expression string -> typed `Query` AST | none |
| `flecs-query/mcp` | `ix-flecs-query-mcp` (pkg `flecs-query-mcp`) | stdio MCP server | `ix-flecs-query-mcp` |
| `flecs-query/py` | `flecs-query-py` | PyO3 bindings returning plain dicts | none |

All three are Rust workspace members; only `mcp` is a flake/packageSet output
(`packages/flecs-query/mcp/default.nix`, binary `ix-flecs-query-mcp`,
`nix run .#ix-flecs-query-mcp`). `core` has no required dependencies; the
optional `serde` feature (enabled by `mcp` and `py`) derives serialization of
the AST.

## core (`flecs-query-core`)

`parse(&str) -> Result<Query, ParseError>` (`core/src/parser.rs`, re-exported
`core/src/lib.rs:86`); `Query` also implements `FromStr` (`lib.rs:88-93`). The
AST (`core/src/ast.rs`) covers terms, access modifiers, operators, pairs,
traversal flags, equality terms, and references: public types `Access`, `EqOp`,
`EqOperand`, `EqTerm`, `ExtraOper`, `IdFlag`, `IdTerm`, `Oper`, `Query`, `Ref`,
`RefExpr`, `Src`, `Term`, `TermBody`, `Traversal` (`lib.rs:81-84`). `Display`
(`fmt.rs`) renders the canonical form (normalized whitespace, comments dropped,
implicit-source pairs in `(Rel, Tgt)` shape). Errors carry a byte `Span` and a
caret-rendered message (`error.rs`, `ParseError::render`).

The EBNF grammar, reverse-engineered from upstream's parser and test suite, is
in the crate docs (`core/src/lib.rs:18-36`). It scopes to **form, not
resolution**: `parse` answers "is this well-formed Flecs Query Language" and
gives every term its structure, but whether `Position` names a real component is
a property of a specific world that a parser without a world cannot and should
not guess (`lib.rs:45-52`). It deliberately rejects two things upstream silently
ignores: an unknown access modifier (`[foo]`) and an unknown word where a
traversal flag belongs (`lib.rs:54-60`).

## mcp (`ix-flecs-query-mcp`)

A stateless stdio MCP server built on `rmcp` (`mcp/src/main.rs`), serving three
tools, each taking one `expr` string (`ExprArgs`, `main.rs:31-36`):

- `parse`: return the typed AST as JSON; a syntax error is an
  `INVALID_PARAMS` error with the structured `ParseError` and a caret message
  (`main.rs:63-78`, `parse_expr`).
- `canonicalize`: return the normalized expression text (`main.rs:80-87`).
- `validate`: never errors; returns `{valid, error?, rendered?}` for linting
  loops (`main.rs:89-114`).

`get_info` advertises the server as `ix-flecs-query-mcp` with tools enabled and
states that parsing is world-independent (`main.rs:128-144`). Transport is
`rmcp::transport::stdio` over a tokio runtime (`main.rs:17-22`).

## py (`flecs-query-py`)

PyO3 `cdylib`, module `_flecs_query` (`py/src/lib.rs`): `parse` (AST as plain
dicts via `pythonize`, raising `ValueError` with a caret message on error),
`canonicalize` (normalized text), and `validate` (a non-raising
`{valid, error?, rendered?}` dict). Not a flake output; built and bundled into
the kernel like the other `*-py` crates.
