# DSL: the `.sym` workflow language

`elixir/lib/symphony_elixir/dsl/` is the front end that turns `.sym` source into
the durable IR graph the [engine](../engine/overview.md) walks. The pipeline has
three representations (`dsl/ast.ex:5-13`):

```
.sym source --parse--> reified AST --interpret/expand--> IR.Node delta
  Parser/Lexer          AST              Interpreter
```

Everything is plain data, never a host closure: a closure could not be serialized
into the durable `RunGraph`, shown in the dashboard, or replayed deterministically
after a restart, so the whole surface is reified as structs the interpreter walks.

## Modules

- **`parser/lexer.ex`** - tokenizes source into a flat list, each token carrying a
  1-based line/column for precise diagnostics (`lexer.ex:1-17`).
- **`parser.ex`** - the recursive-descent parser; `parse/2` returns `{:ok, ast}`
  or `{:error, diagnostic}` with a source span (`parser.ex:1-12`).
- **`ast.ex`** - the reified constructors and types (workflow, bind, let, effects,
  combinators, pure values).
- **`interpreter.ex`** - `expand/3`: the eval step that emits `IR.Node`s.
- **`schema.ex`** - one JSON-able snapshot of the runtime's enum vocabulary, fed
  to the dashboard forms (`schema.ex:1-12`).

## Lexer (`parser/lexer.ex`)

Keywords (`lexer.ex:19`): `workflow agent exec subrun when every of map as skill
inline timeout true false null`. Punctuation tokens: `<-` (`:larrow`), `{` `}`
`[` `]` `:` `,` `=`. Three non-obvious decisions (`lexer.ex:7-16`):

- String literals are emitted as segment lists of `{:lit, text}` and `{:ref,
  path}`, so `"summarize ${session.report}"` tokenizes with the interpolation
  already split out.
- A bare `${path}` outside a string is its own `:interp` token (the common
  `when ${gate.ok} { ... }` shape).
- `#` starts a line comment to end of line. `\n`/`\t`/`\r`/`"`/`\\` escapes are
  recognized in strings (`lexer.ex:154-159`); numbers lex to `:int` or `:float`.

## Grammar

A workflow is an optional name and trigger header, then a brace `do`-block of
statements (`parser.ex:122-132`):

```
workflow [ "<name>" ] [ on <trigger> ] { stmt* }
stmt    := name "<-" expr      # bind: name binds an effect's output
         | name "=" pure       # let: name binds a pure value
         | expr                # a bare effect
expr    := effect | pure
```

Effects (the only constructors that become IR nodes, `ast.ex:29-37`):

```
agent  { engine: .. model: .. effort: .. permissions: .. location: ..
         inputs: { k: pure } prompt: <prompt_ref> }
exec   <pure> [ timeout <int> ] [ { k: pure } ]
subrun <pure> [ { k: pure } ]
```

Combinators (dynamic expansion, `ast.ex:44-59`), each body is a brace block with
exactly one statement (`parser.ex:483-492`):

```
when <pure> { stmt }                # run body only if pure is truthy
every <int> of <counter> { stmt }   # run body every nth tick of a named counter
map <pure> as <name> { stmt }       # fan the body out once per list element
```

Pure values (`ast.ex:61-72`, `parser.ex:539-568`): string literals (with `${path}`
interpolation), integers, floats, `true`/`false`/`null`, bracketed lists `[a, b]`,
and `${name.field.path}` references. A bare `${name}` lowers to a `var`; a dotted
`${name.a.b}` lowers to a `field` read; an interpolated string lowers to a
`concat` of literal and field segments (`parser.ex:593-615`).

A `prompt:` field is either `skill "name" [ { bindings } ]` or `inline <pure>`
(`parser.ex:384-409`). The agent envelope keys are exactly `engine model effort
permissions location` (`parser.ex:330`); an unknown agent field or a missing
`prompt` is a located error (`parser.ex:364-365`, `parser.ex:619-620`).

### Example

`elixir/test/symphony_elixir/dsl/fixtures/release.sym`:

```
workflow "release" {
  inspect <- agent {
    engine: codex
    model: "gpt-5.3-codex"
    effort: medium
    permissions: workspace_write
    location: local
    prompt: skill "inspect" { repo: "symphony" }
  }

  report <- agent {
    engine: claude
    model: haiku
    permissions: read_only
    prompt: inline "write a status report and stop"
  }

  summary <- agent {
    engine: codex
    model: "gpt-5.3-codex"
    permissions: read_only
    prompt: inline "summarize ${inspect.area}"
  }

  when ${inspect.changed} {
    notify <- exec "./scripts/notify.sh" timeout 30
  }
}
```

`inspect` and `report` read disjoint inputs, so they have no edge and run in
parallel; `summary` reads `${inspect.area}`, so it waits on `inspect`.

## Triggers (the `on` header)

The header `on <kind> <params>` declares what fires the workflow; omitting it
yields a `nil` trigger (an operator-only workflow). The kinds are the source of
truth in `parser.ex:80` (`@trigger_kinds`), exposed through `Parser.trigger_kinds/0`
so the [schema](#schema-vocabulary) and dashboard forms offer exactly what the
parser accepts:

| kind | surface | normalized map (`parser.ex:156-189`) |
| --- | --- | --- |
| `manual` | `on manual` | `%{kind: :manual}` |
| `cron` | `on cron "<sched>" [tz "<zone>"] [input {..}]` | `%{kind: :cron, schedule, timezone, input}` |
| `linear` | `on linear label "<label>"` | `%{kind: :linear, label}` (downcased) |
| `slack_huddle` | `on slack_huddle channel "<ch>"` | `%{kind: :slack_huddle_completed, channel}` |
| `slack_mention` | `on slack_mention channel "<ch>"` | `%{kind: :slack_app_mention, channel}` |
| `github_pr_label` | `on github_pr_label repo "<r>" label "<l>"` | `%{kind: :github_pr_label, repo, label}` |

The normalized maps match the runtime's trigger shapes so a producer can match an
inbound event against a workflow with one shared matcher
([Runtime.Trigger](../engine/overview.md#triggers-and-ingress)). The `cron`
schedule grammar is parsed separately by `CronExpression`
(see [operations](../operations/overview.md#triggers)).

## Interpreter: expand (`interpreter.ex`)

`expand(ast, known_outputs, expansion_log)` returns `{ir_delta, pending, new_log}`
(`interpreter.ex:7-20`):

- `ir_delta` is the list of `IR.Node`s to materialize this pass. Re-running with
  more `known_outputs` yields the next delta.
- `pending` is `{:awaiting, ast_id, needed_node_ids}` for effects that cannot
  materialize until upstream outputs arrive (a prompt or input mixing a literal
  with a node read cannot be one input ref, so it is deferred, `interpreter.ex:41-51`).
- `new_log` extends the expansion log with one event per dynamic expansion that
  fired; its order is load-bearing for replay.

The four rules (`interpreter.ex:22-39`):

1. Only `agent`/`exec`/`subrun` become nodes; pure computation is evaluated here.
2. `IR.Node.deps` is derived from `inputs`; the interpreter never hand-writes edges.
3. `when`/`every`/`map` emit a `:gate` or `:map_fanout` placeholder when their
   gating input is unresolved, and emit children deterministically when re-expanded
   with the resolved output.
4. Gates are pure functions of `known_outputs` and counters recovered from the
   log: no wall clock, no RNG. The same `(ast, known_outputs, expansion_log)`
   always yields the same `ir_delta`.

### Pure value resolution

`resolve_value/3` reduces a pure to `{:value, v}`, `{:node, id, path}` (the one
shape that makes a dependency edge), or `:deferred` (`interpreter.ex:433-490`). A
fully-literal value folds to a literal; a single `${node.path}` read becomes a
node ref; a `concat`/`list` mixing literals with a node read is deferred until the
referenced outputs are known, then folds on re-expansion.

### Combinator semantics

- `when`: evaluates `cond`; emits the body if truthy (`true`/non-nil), else logs a
  closed gate and emits nothing (`interpreter.ex:193-210`).
- `every n of counter`: deterministic gate keyed on a persisted counter recovered
  from the log; fires when the next tick is a multiple of `n`. To keep a re-pass
  within one run idempotent, an already-recorded tick is reproduced from the log
  rather than recomputed (`interpreter.ex:212-272`).
- `map over as elem`: fans the body out once per list element with a stable
  `{:fanout, id, index}` expansion key, binding each element to `as`. A non-list
  `over` is a typed mismatch surfaced as an empty fan-out, not a crash
  (`interpreter.ex:235-339`). A `name <- map ...` binding is dropped: a fan-out
  binds no single node (`interpreter.ex:316-318`).

## Stable node ids and diagnostics

The parser assigns each effect a positional id (`agent-0`, `exec-1`, ...) in
source pre-order (`parser.ex:688-695`); the interpreter combines it with an
expansion key to derive the final `IR.Node` id, so re-parsing identical source
yields identical ids, the property replay needs (`ast.ex:74-79`). Every diagnostic
is `%{message, line, column, file, got}` with a 1-based span pointing at the
failing token; `file` is stamped by the `WorkflowCatalog` so an author sees which
`.sym` broke (`parser.ex:53-61`).

## Schema vocabulary

`Schema.to_map/0` (`dsl/schema.ex`) collects the runtime's enums into one map: `engines`, `efforts`,
`permissions`, `locations` (from `Engine.Envelope`), `node_kinds`/`node_states`
(from `IR.Node`), `effect_kinds` (from `AST`), and `trigger_kinds` (from
`Parser`). Each field reads the single accessor on the module that owns the enum,
so the schema cannot drift from what a turn, node, or the parser will accept
(`schema.ex:6-11`). It is served at `GET /api/v1/ir/schema` and drives the
dashboard's option lists.
