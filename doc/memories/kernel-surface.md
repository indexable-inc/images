# memories: the kernel surface

`IxMcp.Memories` is the Elixir side of `.memories` (index#4433): the
`memories` CLI wrapped as an in-cell callable, aliased `Memories` in the
index kernel's workspace prelude
(`packages/mcp-ex/lib/ix_mcp/workspace.ex:35`). Everything the agent-facing
surface can do is one `memories` invocation; the CLI owns the on-disk
format, and nothing here parses or renders frontmatter.

For the file format, the ranking function, the lint rules and the CLI
itself, see `packages/memories/CONTRACT.md`.

## What a caller gets

```elixir
Memories.search("why did every host rebuild")             # %Memories.Results{}
Memories.search("rebuild", topic: :nix, limit: 5,
                dirs: ["/a/.memories", "/b/.memories"])   # these roots, not the default set
Memories.roots()                                          # %Root{path, exists, memories} rows
Memories.expand(results.hits, depth: 1)                   # + `related:` neighbours
Memories.show("nix-rebuild-cascade")                      # one hit, no ranking fields
Memories.stale() / Memories.refuted() / Memories.unchecked(days: 30)
Memories.lint()                                           # %{diagnostics:, errors:, checked:}

Memories.remember("nix-rebuild-cascade", "one-line tldr",
  body: markdown, topic: [:nix], handle: ~w(nix-dag),
  based_on: ["packages/nix-dag/src/rank.rs"],
  by: "claude-opus-5", how: "nix-dag .#hil-compute-2")   # by:/how: required for genre: :memory
Memories.validate("nix-rebuild-cascade", by: "claude-opus-5", how: "the command")
Memories.refute("nix-rebuild-cascade", by: "claude-opus-5", how: "...", instead: "new-slug")
```

## A result says where it looked

`search/2` returns `%IxMcp.Memories.Results{}`, not a bare list: `hits`, and
next to them the `roots` it resolved, the `scanned` count, `elapsed_ms` and
the `query`. Zero hits from a root set that silently resolved to one
unexpected directory is indistinguishable from zero hits from the right
directories, and the hits alone cannot tell the two apart -- so the roots
ride with the result instead of waiting for a caller to think of asking.
`roots/1` answers the same question with no query.

A root is a `%IxMcp.Memories.Root{path, exists, memories}` row, not a path,
and the count is the point: flattening these rows back to paths would delete
the distinction the section above is about. `memories: 0` on every row says
the search covered nothing; a healthy count next to zero hits says the
corpus genuinely has no good answer. `exists: false` is normal for a default
root (a repo with no `.memories` yet is skipped) and is a typo when the
caller named that root itself, which the CLI refuses outright.

**An empty result is not an error.** The CLI drops hits below a score floor
rather than returning its best of a bad set, so `hits: []` against healthy
roots means "nothing good here" and should be reported as such. Nothing in
this module treats it as a failure.

`expand/2` takes and returns hits, so it is `Memories.expand(results.hits)`.

## Root sets are plural

`dirs:` is a list of `.memories` directories, and it **replaces** the default
resolution rather than adding to it: a caller naming its roots reads exactly
those and inherits nothing, because "adds to" would return results from
directories nobody named. It is a list even when it names one directory, and
a bare string raises (`memories.ex:594-614`) -- one spelling means the plural
path is the tested path.

## A durable memory is born with its proof

`remember/3` requires `by:` and `how:` for the default `genre: :memory`, and
the CLI writes them as the file's first `validated` entry, so one call
produces a memory that already passes `lint` instead of one that fails
`memory-unchecked` from birth. A `:living`, `:recipe`, `:historical` or
`:frozen` page needs neither.

This module refuses the memory-genre call without them (`proof!/1`) rather
than letting the CLI exit 2, because the raise can name both options and the
genre while a subprocess usage error cannot. Quote a `how:` that contains a
colon -- see the lint section.

## Ranked once, never twice

`search/2` returns hits in the CLI's ranked order and there is deliberately
no `rank/1` (`memories.ex:336`). Two names for one result is how a caller
ends up sorting twice; a caller that wants the unranked lexical order reads
`bm25` off each hit and sorts it itself. Ties break by score descending then
slug ascending, which is what makes `limit:` reproducible across runs, so
re-sorting would throw that away too. Nothing in this module reorders what
the CLI ranked, which the test pins by having the mock emit hits whose
`score` and `bm25` both ascend: a wrapper that sorted either field would
reverse that order
(`packages/mcp-ex/test/ix_mcp/memories_test.exs:81-84`).

## Hits are structs, not maps

`search/2`, `show/2` and `expand/2` decode hits into `%IxMcp.Memories.Hit{}`
(`memories.ex:88`), whose `validated` receipts are `%Memories.Validation{}`
with a real `DateTime` (`memories.ex:51`). The review commands return
`%Memories.Row{}` (`memories.ex:254`), lint findings are
`%Memories.Diagnostic{}` (`memories.ex:278`), and a search result's roots are
`%Memories.Root{}` (`memories.ex:182`). `IxMcp.Memory.recall/2` returns bare
maps and this is the deliberate departure from it: a key the CLI stops
emitting raises at the boundary via `Map.fetch!/2` instead of surfacing as a
`nil` three calls later.

Two fields are absent rather than null by contract, so they decode with
`Map.get/2` and are nil on a `show`: `bm25` and `score`
(`memories.ex:157-158`). `genre` decodes to one of five atoms through
spelled-out clauses (`memories.ex:170-180`) -- not
`String.to_existing_atom/1` -- so an unknown genre names the file it came
from. `scope` stays a string (`"shared"` or `"user:<name>"`), because the
user half is open-ended.

## The working directory is part of the query

Discovery is relative to the CLI's working directory: the `.memories` of its
git toplevel, then each parent toplevel, then `~/.memories`. The OS cwd is
BEAM-global and any cell can move it (#3902), so every call goes through
`IxMcp.Cmd.run/3` and inherits the kernel's immutable launch directory unless
the caller passes `cd:` (`memories.ex:616`).

`expand/2` reads each neighbour from the `.memories` **root** its referrer
came from (`memories_dir/1`, `memories.ex:543-553`), not from the cwd and not
from `Path.dirname(hit.path)`: memories nest one level deep in grouping
subdirectories (`.memories/cas/cas-gc-proof.md`), and a group is not a root,
so dirnaming a nested hit would search that group and hide every sibling.
The derivation uses `hit.root`, which is why a slug being a file stem rather
than a path matters here -- the neighbour resolves wherever in the root it
lives. The `memories_test.exs` case for this fails with
`--dir /repo/.memories/hardware` if the derivation regresses.

A `related:` slug that does not resolve raises; `lint/1` reports the same
thing as `memory-related-unresolved`.

## Argv, exit codes, and the one shell hop

Arguments are built as `<subcommand> [--dir D]... --json [flags] --
<positional>` (`memories.ex:588`). The `--` matters: a query or slug starting
with `-` is data, not a flag. Options this module was not given are not
passed at all, so `--limit` and `--days` have exactly one default and it
lives in the CLI.

Exit 0 is success everywhere except `lint`, where the CLI reports a lint
error as exit 1 with the report still on stdout; `lint/1` therefore accepts
`[0, 1]` and returns diagnostics rather than raising (`memories.ex:414`).
Any other nonzero status raises with the argv and the CLI's own output, so a
slug that does not resolve surfaces the CLI's message.

`remember` takes its body on stdin, and an Erlang port's stdin is a pipe the
BEAM never closes and cannot half-close. The body goes to a temp file that an
inner `sh` redirects from, still spawned through `IxMcp.Cmd.run/3` so the
launch-directory default and its cwd checks hold; the path travels in the
environment, so no quoting of it can reach the shell (`memories.ex:641`).
A `--body-file` on the CLI would delete this hop.

## Lint rules worth knowing about as a caller

`lint/1` returns every diagnostic; two rules are scoped, and the scoping is
why the format has no `evergreen` escape hatch: `memory-body-budget` and
`memory-unchecked` fire on `genre: memory` only, so a long reference page and
its 180-day validation clock are not findings. `memory-directory-budget`
counts per leaf directory rather than per root, which is what makes the
nested layout viable, and `memory-stem-collision` is what keeps stems unique
per root so `show/2` can stay a lookup.

`memory-secret` matters more here than the rule list suggests, because
`remember/3`'s whole point is pasting the exact command into `validated[].how`
and commands carry tokens. Measured against the CLI (2026-07-29): it fires on
AWS keys, GitHub tokens, `sk-` keys and a `lin_api_` token inside a `how:`
line -- but only when the frontmatter parses. The same token written the way
an agent would actually write it, unquoted:

```yaml
    how: curl -H "Authorization: lin_api_abc123DEADBEEFcafe0987654321" ...
```

is invalid YAML (the bare `: ` opens a nested mapping), and that file reports
`memory-frontmatter` alone -- no `memory-secret`. So the input most likely to
carry a credential is the one where the credential is not flagged. Quote the
`how:` value when it contains a colon, which also makes the memory lint-clean.

## Setup

`MEMORIES_BIN`, then `memories` on `PATH`, then a loud error naming the knob
(`memories.ex:516`) -- the same shape as `IxMcp.Memory.weave_bin!/0`. There
is no store path to configure: the directories are found from the repo.

## Verified against the real binary

Beyond the mock, the whole surface was run against `nix build .#memories` in
a throwaway git repo, rebuilt as the CLI landed (2026-07-29). Green end to
end, including the two calls that needed the newest CLI:

```
== roots rows on the result: [%Root{path: ".../.memories", exists: true, memories: 2}]
== roots/1: [%Root{path: ".../.memories", exists: true, memories: 2},
             %Root{path: "/Users/andrewgazelka/.memories", exists: false, memories: 0}]
== hit: %{slug: "nix-rebuild-cascade", scope: "shared", genre: :memory, topic: ["nix"],
          handle: ["nix-dag", "drvPath"], prior: 0.8, stale: false, refuted: false}
== below-floor query is an answer, not an error: {[], [true: 2]}
== receipt: %Validation{at: ~U[2026-07-30 02:54:23Z], by: "claude-opus-5",
                        how: "nix-dag .#hil-compute-2", ok: true}
== refuted excluded unless all:: {["nix-rebuild-cascade"],
                                  ["nix-eval-before-deploy", "nix-rebuild-cascade"]}
```

That last-but-two line is the root rows earning their keep: zero hits next to
`exists: true, memories: 2` is a genuine miss under the score floor, which a
bare path list could not have said.

Also verified against the binary: a body on stdin round-tripping byte for
byte, `--scope` writing `"user:andrew"` and `show/2` reading it back, the
nested `.memories/hardware/` layout, `bm25`/`score` present on `search` and
absent on `show`, `expand/2` resolving a `related:` neighbour from the root,
`lint/1` on its exit-1 path, and the `stale`/`refuted`/`unchecked` rows.

## Tests

`mix test test/ix_mcp/memories_test.exs` runs 14 tests with no real binary:
`test/fixtures/memories_mock.exs` is a stand-in CLI that logs its argv (and,
for `remember`, the body it read on stdin) and answers with contract JSON,
spawned through a `/bin/sh` trampoline rather than its own shebang for the
reason `IxMcp.Memory.SemanticTest` documents. The suite pins argv
construction per subcommand, struct decoding, ranked-order preservation, the
exit-1 lint path, the missing-binary error, and `expand/2`'s
no-refetch/append behaviour.

## Nothing arrives unasked

There is no session-start injection and no `always:` field, so a memory
reaches a model exactly one way: it searched. That is measured, not a
preference -- `docs/_archive/design/context-research.html` (2026-06-12, 14
agents, live A/B on this fleet) found deliberate prior-search paying 4 to 8
times over, 53k injected tokens against 220-400k tokens of avoided
rediscovery, while ambient injection into ordinary prompts was net-negative:
3 of 5 casual prompts pulled 0.3 to 9k tokens of pure noise, breaking even
only score-gated at 0.70 with a ~1200-token cap. Its conclusion: "Session-start
digests must come from distilled facts, never live vector hits."

So the weave-backed memory digest in
`users/andrewgazelka/profiles/workstation.nix` was deleted rather than
reimplemented over `.memories`, and nothing replaced it. The `session-digest`
hook (`packages/claude-hooks/src/main.rs:195`) reads
`~/.cache/ix/context-digest.md`, caps at 6000 chars and stays silent when the
file is absent, so with no producer it simply goes quiet; nothing here writes
that file.

`IxMcp.Memory` and its weave store are untouched by all of this and remain
callable: `WEAVE_MEMORY_STORE` stays set, so the memories already in that
store stay readable.
