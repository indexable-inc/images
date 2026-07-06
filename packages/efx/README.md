# efx

A content-addressed effect engine: Terraform's plan/apply split treated as a
small calculus, or Nix derivations generalized to arbitrary effects.

An **effect** declares its kind, executor, inputs, and idempotence metadata.
Its identity is the SHA-256 of the canonical serialization of
`(kind, executor, resolved input hashes)`, where a reference input hashes as
the *identity of the effect it reads from* — so changing any input re-identifies
the effect and, transitively, everything downstream. The **journal** (state
file) maps identities to recorded outputs, which makes it the memoization
cache: `plan` is "which ids are missing from the journal", `apply` executes
exactly those, in parallel across independent effects.

## Crates

| Crate | What it is |
| --- | --- |
| [`ir/`](ir/) | The plan IR: effects, literal/reference inputs, dataflow edges, content-addressed `EffectId` |
| [`engine/`](engine/) | Journal, plan diff (with invalidation reasons and orphan reporting), level-parallel apply via an `Executor` registry |
| [`lang/`](lang/) | The `.efx` surface language — total by construction (no loops, recursion, or conditionals), compiles to the IR |
| [`cli/`](cli/) | `efx plan` / `efx apply` / `efx report --html`, with `file.write`, `cmd.run`, and `html.render` executors |

Each layer stands alone: any program can build `efx_ir::Plan`s directly and
hand them to the engine; the language is one frontend, not the contract.

## The language

```text
let title = "hello from efx"

effect stamp "cmd.run" {
  command = "echo built by the efx demo"
}

effect page "html.render" {
  template = "<h1>{title}</h1><p>{{stamp}}</p>"
  stamp = ref("stamp").stdout
}

effect site "file.write" {
  @rollback = "remove out/index.html"
  path = "out/index.html"
  content = ref("page").html
}
```

`{name}` interpolates earlier `let` bindings at compile time; `ref("x").field`
wires an upstream output into an input at execution time; `@idempotent` and
`@rollback` set metadata. Bindings only see earlier bindings, so every
program terminates.

## Demo

```sh
./cli/examples/demo.sh /tmp/efx-demo /tmp/efx-demo.html
```

Applies [`cli/examples/site.efx`](cli/examples/site.efx) (everything executes),
applies again (all cache hits), retitles the page and re-plans (only the
changed effect and its dependents invalidate), then renders the run history —
DAG, cache hits vs executions, and what invalidated — as one self-contained
HTML file.

Out of scope, deliberately: remote state, locking, and rollback *execution*
(`@rollback` is carried as metadata only).
