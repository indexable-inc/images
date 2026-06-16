# Pack: the workflow-pack format

The runtime is pack-agnostic: workflow shape lives in a pack directory, not in
`elixir/lib/` (`packages/symphony/AGENTS.md:29-35`). A pack is three things:

```
<pack>/
  workflows/   one .sym file per workflow (hot-reloaded by WorkflowCatalog)
  skills/      one .md system prompt per skill (hot-reloaded by Catalog)
    _partials/ shared markdown fragments included with {{partial:name}}
  repositories.yaml   the repos cloned into each run's workspace
```

The active pack is chosen by `SYMPHONY_PACK_DIR` (an external directory) or
`SYMPHONY_WORKFLOW_PACK` (a pack name under the symphony repo's `workflows/`,
default `example`). Each subpath is overridable individually
(`SYMPHONY_WORKFLOWS_DIR`, `SYMPHONY_SKILLS_DIR`, `SYMPHONY_REPOSITORIES_FILE`)
and defaults under the pack dir (`config.ex:16-24`). `Config` validates at boot
that the pack dir, workflows dir, skills dir, and repositories file all exist,
raising a clear error otherwise (`config.ex:548-571`). Symphony treats the pack
as read-only runtime input; mutable state goes under `SYMPHONY_RUNS_DIR` and
`SYMPHONY_WORKSPACES_DIR` (`packages/symphony/docs/setup.md:36-38`).

## Workflows (`workflows/*.sym`, `workflow_catalog.ex`)

Each `.sym` file is one workflow in the [DSL](../dsl/overview.md). The
`WorkflowCatalog` GenServer watches `workflows/*.sym` and publishes the latest
parsed AST per file, polling every `SYMPHONY_CATALOG_POLL_MS` (default 1000ms) and
comparing SHA-256 hashes (`workflow_catalog.ex:1-29`). Reload semantics: a new file
is parsed and added, changed bytes re-parsed, a deleted file removed; a parse
error keeps the last-good entry published and records the located diagnostic
(message, line, column, file) so the workflows view shows exactly where one `.sym`
broke while the rest keep working (`workflow_catalog.ex:22-28`, `153-180`).

Entries are keyed by file basename and carry `name`, `ast`, the declared `trigger`
(lifted from the AST for cheap matching), the raw `source`, and the `hash` a run
records as `RunGraph.source_hash` (`workflow_catalog.ex:42-49`). A run snapshots
that hash at start, so editing the pack only affects new runs. `for_trigger_kind/1`
is the producer's first filter; `workflow/1` resolves one by name for the manual
path. The catalog owns parsing and freshness only; it never starts runs.

## Skills (`skills/*.md`, `catalog.ex`, `skill.ex`)

A skill is a markdown file whose body is the system prompt for an agent node, with
optional YAML frontmatter in the model-agnostic Agent Skills shape: a one-line
`description` and a `tools` allowlist (`skill.ex:1-6`, `72-83`). The skill carries
no engine, model, effort, or permissions: those are the node's
[envelope](../engine/contract.md#envelope), so the skill body stays
model-agnostic (`skill.ex:21-27`). The body is the lever for improving an agent
without code changes. The bundled `skills/inspect.md`:

```markdown
---
description: Sample skill that inspects the checked-out workspace and reports, without mutating anything.
tools: []
---

You are running inside a sample Symphony workflow.
Read the input and inspect the checked-out workspace. ...
```

The `Catalog` GenServer watches `skills/*.md` non-recursively, hot-reloading on
the same 1s hash-compare tick (`catalog.ex:1-22`). `Skill.load/1` splits
frontmatter, decodes the YAML, and expands shared partials at load time. Active
runs snapshot the skills they resolve at run start; a reload affects only new runs.

### Partials (`skills/_partials/*.md`)

A skill body includes a shared fragment by writing `{{partial:<name>}}`, resolved
to `skills/_partials/<name>.md` (`skill.ex:29-43`). Files under `_partials/` are
not skills: the catalog globs `*.md` non-recursively, so they are ignored.
Expansion runs to a fixpoint with a seen-set so each named partial inlines at most
once and a partial that documents its own token name does not loop or leave a
residual token; a genuinely missing partial is a load error so the catalog refuses
to publish a half-rendered body (`skill.ex:45-71`). Partials are not hot-reloaded
on their own; touch a referencing skill to pick up a partial edit.

## Prompt rendering (`prompt.ex`)

`SymphonyElixir.Prompt.build/2` turns a node's `prompt_ref` into the text an engine
runs (`prompt.ex:1-24`). `{:inline, text}` passes through; `{:skill, name,
bindings}` loads the body through an injected resolver (production: the `Catalog`),
expands any `{{partial:name}}`, and interpolates `${path}` placeholders from the
bindings the interpreter resolved (`${ticket.id}` reads `bindings["ticket"]["id"]`).
A placeholder with no matching binding is a render error, not a silent empty
substitution, so a skill referencing an input the node never bound fails loudly
(`prompt.ex:26-33`). Write `$${path}` to emit a literal `${path}`: skill bodies
routinely embed shell/Make/JS `${VAR}` snippets the doubled `$$` protects
(`prompt.ex:35-39`). At turn time `RoomEngineClient` appends the run's trigger as
an `<input>` JSON block so a dispatch-driven skill reads its payload
(`runtime/room_engine_client.ex:96-108`).

## Repositories (`repositories.yaml`, `repository_catalog.ex`)

`repositories.yaml` lists the repos cloned into every run's workspace. Each entry
is `name`, `owner_repo`, `default_branch`, and an optional `primary` flag; exactly
one repo must be `primary: true` (`repository_catalog.ex:30-49`). The bundled
`workflows/example/repositories.yaml`:

```yaml
repositories:
  - name: example
    owner_repo: example/example
    default_branch: main
    primary: true
```

`Workspace.create/1` clones every listed repo for each run under
`SYMPHONY_WORKSPACES_DIR/<run_id>/`, the primary at the run cwd and siblings
beside it, each with writable refs and a run-scoped branch so an agent can branch,
commit, and open PRs in any listed repo (`workspace.ex:1-17`,
`repository_catalog.ex:2-9`). Workspace paths are canonicalized under the
workspaces root, rejecting symlink escapes (`workspace.ex:66-85`).

## The bundled example pack (`workflows/example/`)

The public default, intentionally narrow: a single manual-trigger workflow plus a
read-only skill that pushes nothing anywhere, meant as a starting point to copy
(`packages/symphony/docs/setup.md:15-18`). `workflows/example/workflows/inspect.sym`:

```
workflow "inspect" on manual {
  inspect <- agent {
    engine: codex
    model: "gpt-5.3-codex"
    effort: medium
    permissions: workspace_write
    prompt: skill "inspect"
  }
}
```

It pairs with `skills/inspect.md` (above) and the one-repo `repositories.yaml`.
Real deployments point `SYMPHONY_PACK_DIR` at their own pack; keeping the example
narrow is a deliberate constraint so no workflow names, repo slugs, labels, or
ticket schemes leak into the core (`packages/symphony/AGENTS.md:29-35`). See
[operations](../operations/overview.md) for running a pack.
