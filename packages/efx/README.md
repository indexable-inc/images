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
| [`cli/`](cli/) | `efx plan` / `efx apply` (over `.efx` files or `--ir` plan JSON) / `efx report --html`, with local and Cloudflare executors |

Each layer stands alone: any program can build `efx_ir::Plan`s directly and
hand them to the engine; the language is one frontend, not the contract.
`efx plan --ir plan.json` / `efx apply --ir plan.json` accept the IR document
itself, which is how the nix frontend below plugs in.

## The nix frontend: replacing terranix

The infra stacks that used to render terraform JSON through terranix render
efx plan IR instead: nix stays the Turing-complete generator (inventory-derived
DNS records, cross-file constants), `lib/util/efx.nix` (exposed as
`index.lib.efx`) builds the document, and the efx engine replaces the
tofu plan/apply/state machinery — the journal is the state file.

```nix
efx.plan (efx.fromTerranix {config = cloudflareStack;}
  ++ [(efx.effect {
    name = "heartbeats_json";
    kind = "file.write";
    inputs = {
      path = "generated/heartbeats.json";
      content = efx.ref "heartbeats_render" "html";
    };
  })])
```

`fromTerranix` consumes the terranix-shaped `resource.<type>.<name>` config
unchanged: `"${cloudflare_zone.ix_dev.id}"` interpolations become efx
references (so the DAG and invalidation come for free), nested attrsets
flatten to dotted input keys, lists become canonical-JSON string inputs, and
`terraform`/`provider`/`import` blocks are dropped — state is the journal,
auth is executor environment. Anything without a faithful IR encoding
(floats, interpolations embedded mid-string or inside structured values)
throws at eval time; the translation is total-or-loud.

Parity is pinned by one artifact: tests/efx in the repo root ports the ix
terraform stacks (Cloudflare, OVH, Better Stack) against a fixture inventory,
the nix eval tests assert the rendered plan equals
[`cli/tests/fixtures/terranix_port.plan.json`](cli/tests/fixtures/terranix_port.plan.json),
and the CLI tests parse and plan that same file.

## Executors

Real, reconciling (create / update-in-place / converge, refusing loudly on
ambiguity — efx never destroys, orphans are reported instead):

- `file.write`, `cmd.run`, `html.render` — the local surface
- `cloudflare.zone`, `cloudflare.dns_record` (with an explicit
  `strategy = "ensure"` mode for set-typed records like MX/CAA),
  `cloudflare.r2_bucket`, `cloudflare.workers_route` — via `curl` against the
  v4 API; `CLOUDFLARE_API_TOKEN` from the environment, `CLOUDFLARE_API_BASE`
  overridable (the integration tests run against a local stub)

Declared gaps — planned fully, but `efx apply` fails them with an explicit
"not implemented, resource NOT applied, keep using the opentofu stack"
error rather than pretending to reconcile: `cloudflare.ruleset`,
`cloudflare.email_routing_*`, `cloudflare.r2_managed_domain`,
`ovh.dedicated_server`, and the `betteruptime.*` kinds (see
`register_declared_gaps` in [`cli/src/executors.rs`](cli/src/executors.rs)).

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

Out of scope, deliberately: remote state, locking, rollback *execution*
(`@rollback` is carried as metadata only), and destroy — an effect that
leaves the plan is reported as a journal orphan, never deleted.
