# .memories contract (index#4433)

The interface both the Rust CLI and the Elixir kernel surface build against.
Frozen before either was written so the two could be built in parallel.

## File format

One memory per file: `<repo>/.memories/[<group>/]<slug>.md`, YAML
frontmatter then a markdown body. Markdown rather than a whole-file YAML
document because the same format has to hold a 119 KB `ix/docs` reference
page, and a page body inside a YAML scalar is unreadable in a diff and
unrenderable on GitHub.

Subdirectories are allowed one level deep and carry no meaning beyond
keeping any single directory under the file budget: `ix/docs` converts to
221 pages across 26 domains, and a flat directory would fail
`memory-directory-budget` on the day it landed. **The slug is always the
file stem, never the path**, so `related:` and `supersedes:` stay
location-independent and `show <slug>` is a lookup rather than a path
computation. The cost is a redundant prefix on disk
(`.memories/cas/cas-gc-proof.md`), paid deliberately to keep stems globally
unique and checkable.

```markdown
---
tldr: An env var holding a store path makes every dependent rebuild; rank on sole count, not fan-out.
genre: memory            # memory | living | recipe | historical | frozen
topic: [nix, builds]
handle: [nix-dag, drvPath, sole-count]
prior: 0.8               # 0..1, written once at birth, never edited. default 0.5
related: [nix-eval-before-deploy]
based_on:
  - path: packages/nix-dag/src/rank.rs
    blake3: 41ab77c9e0            # content when last validated
validated:
  - at: 2026-07-29T18:22:11Z
    by: claude-opus-4-6
    how: "nix-dag .#hil-compute-2; top sole-count node was IX_ASSETS_DIR"
    ok: true
supersedes: [old-slug]
scope: shared            # shared | user:<name>. default shared
---

Markdown body: why, the evidence, the exact command.
```

`tldr` is the only required field. `genre` defaults to `memory`; `slug`
is the file stem and is never written in the frontmatter.

## Discovery

`readDir` over each `.memories` directory, descending into immediate
subdirectories only, one level deep. No manifest and no index file. The walk
is stated here as well as in the format section because a literal `readDir`
finds zero memories in a nested layout, and the Rust and Elixir sides would
then legitimately disagree about whether `.memories/cas/cas-gc-proof.md`
exists.

A query spans several projects at once, so a root set is plural everywhere:
`--dir` repeats on the CLI and `dirs:` takes a list in Elixir. Passing a
root set overrides the default entirely rather than adding to it, because a
caller naming its roots wants exactly those and nothing inherited.

The default set, when none is given, in order: the `.memories` of the git
toplevel of the cwd, then of each parent git toplevel (submodule before
superproject), then `~/.memories`. A root that does not exist is skipped
silently; a root passed explicitly that does not exist is an error, since
naming a directory that is not there is a typo, not a preference.

Every hit carries `root`, so a caller can tell which project answered.
Slugs are unique per root, not globally: the same slug in two roots is two
memories and both are returned.

**Report the root set with the results.** An empty result from a root set
that resolved to one unexpected directory is indistinguishable from an
empty result from the right directories, and that is how a search tool
silently stops working.

So `roots` is not a list of paths. Each entry is
`{"path": "...", "exists": true, "memories": 137}`, and both `search --json`
and `memories roots` emit the same shape over the same resolved set. Two
weaker designs were considered and both lose something the caller needs:
emitting only the directories actually scanned hides a resolved root that
turned out to be empty, and emitting only the resolved paths hides whether
any of them held anything. With the count present, zero hits against
`memories: 0` everywhere is unmistakably a coverage problem rather than a
genuine miss, and the two commands cannot drift because they print one
function's output.

`memories roots` exists so the resolution can be checked without running a
query at all. The Elixir surface exposes `Memories.roots/1` and carries the
same rows on `Results`.

## Nothing is injected unasked

There is no `always:` field and no session-start injection. Memories reach a
model exactly one way: it searches for them, because the rule in the system
prompt tells it to.

That is the measured call, not a preference. `docs/_archive/design/context-research.html`
(2026-06-12, 14 agents, live A/B) found deliberate prior-search paid 4 to 8
times over on this fleet -- 53k injected tokens against 220-400k tokens of
avoided rediscovery across 10 recurring task types -- while ambient injection
into ordinary prompts was net-negative: 3 of 5 casual prompts pulled 0.3 to 9k
tokens of pure noise, and it only broke even score-gated at 0.70, capped near
1200 tokens, and silent on a miss. Its own conclusion: "Session-start digests
must come from distilled facts, never live vector hits."

So the weave-backed memory digest in `users/andrewgazelka/profiles/workstation.nix`
is deleted rather than reimplemented over `.memories`. The existing
`session-digest` hook (`packages/claude-hooks/src/main.rs:195`, reads
`~/.cache/ix/context-digest.md`, caps at 6000 chars, silent when absent) stays
as it is and nothing here writes that file: it is already correct, and it goes
quiet on its own when no producer exists.

## Ranking

Search returns ranked. There is no separate rank step: a caller that wants
the unranked BM25 order reads `bm25` off each hit and sorts it themselves.

```
score = bm25
      * (0.5 + 0.5 * prior)
      * genre_factor                            # historical|frozen 0.5, else 1.0
      * max(0.3, exp(-age_days / 90))            # age since newest validated.at
      * (1 + 0.15 * ln(1 + n_ok))                # ok:true count
```

Field boosts into BM25: `tldr` 3.0, `handle` 3.0, `topic` 2.0, body 1.0.
Every constant here is a placed guess, not a measurement, and is tuned once
there are enough memories and a handful of queries with known answers.

**`search` returns nothing rather than its best of a bad set.** A hit scoring
below `MIN_SCORE` is dropped, so a query with no good answer comes back empty
and the caller can say so instead of acting on the least-bad match. The
generative-agents-style formula this borrows from has no such floor, and the
one configuration measured net-negative on this fleet was exactly that: always
returning something. The floor is a placed guess like the other constants and
wants the same tuning.

Ties are broken by slug, ascending, after score. BM25 over one-line `tldr`
fields ties far more than BM25 over prose, and the fleet's own reranker was
measured tying five documents at 0.715 and three at 0.663. Without a
deterministic second key, `--limit 3` returns a different three on different
runs and nothing downstream is reproducible.

Excluded from `search` unless `--all`: a memory whose newest `validated`
entry has `ok: false` (refuted). A memory whose `based_on` no longer hashes
to the recorded value is NOT excluded; it is returned with `stale: true`,
because reading a stale memory unflagged is the harm and ranking it low
only hides the flag.

## CLI

`memories` from `packages/memories`. Every subcommand takes repeatable
`--dir <path>` and `--json`; `--json` output is the contract below, human
output is for a terminal and unspecified. Exit 0 on success, 1 on a lint
error or a slug that does not resolve, 2 on a usage error.

```
memories search <query> [--limit 10] [--topic T]... [--genre G]... [--all]
memories roots                                  # the resolved default root set
memories show <slug>
memories stale | refuted | unchecked [--days 180]
memories lint [--fix]
memories remember <slug> --tldr <line> [--genre G] [--topic T]... [--handle H]...
                         [--prior F] [--related S]... [--based-on P]...
                         [--scope shared|user:NAME]   # body on stdin
memories validate <slug> --by <who> --how <cmd> [--not-ok]
memories refute <slug> --by <who> --how <cmd> [--instead <slug>]
```

`remember`, `validate`, `refute` and `lint --fix` write files. The CLI owns
the on-disk format: nothing else renders frontmatter, so the format has one
writer and cannot drift between callers. `validate` recomputes every
`based_on` hash and writes the current value, so validating clears staleness.

### search --json

```json
{
  "query": "why did every host rebuild",
  "roots": [{"path": "/repo/.memories", "exists": true, "memories": 137},
            {"path": "/Users/x/.memories", "exists": false, "memories": 0}],
  "scanned": 137,
  "elapsed_ms": 8,
  "hits": [
    {
      "slug": "nix-rebuild-cascade",
      "path": "/abs/path/.memories/nix-rebuild-cascade.md",
      "root": "/abs/path",
      "tldr": "An env var holding a store path makes every dependent rebuild",
      "genre": "memory",
      "topic": ["nix", "builds"],
      "handle": ["nix-dag", "drvPath"],
      "prior": 0.8,
      "related": ["nix-eval-before-deploy"],
      "supersedes": [],
      "scope": "shared",
      "bm25": 7.412,
      "score": 6.883,
      "stale": false,
      "stale_reason": null,
      "refuted": false,
      "validated": [
        {"at": "2026-07-29T18:22:11Z", "by": "claude-opus-4-6", "how": "nix-dag ...", "ok": true}
      ],
      "body": "A node thousands of derivations depend on is normal ..."
    }
  ]
}
```

`stale_reason` is `null` or a sentence naming the moved path, e.g.
`"based_on moved: packages/nix-dag/src/rank.rs"`. `show` emits one hit
object without the `bm25`/`score` keys. `stale`/`refuted`/`unchecked` emit
`{"slug", "path", "tldr", "reason"}` rows under `"rows"`.

### lint --json

```json
{"diagnostics": [{"path": "...", "line": 3, "rule": "memory-topic-unknown",
                  "message": "topic \"nixos\" is not in the closed set"}],
 "errors": 1, "checked": 137}
```

Rules, all errors: `memory-frontmatter` (does not parse), `memory-tldr`
(missing, empty, or over 1024 chars), `memory-body-budget` (over 3000
estimated tokens at 4 bytes per token, `genre: memory` only), `memory-slug`
(file stem not kebab-case), `memory-topic-unknown` (outside the closed set
in `topics.txt` beside the directory, absent file means any topic),
`memory-related-unresolved`, `memory-duplicate-tldr`,
`memory-supersedes-unresolved`, `memory-based-on-missing` (path does not
exist), `memory-directory-budget` (over 150 files in one `.memories`),
`memory-unchecked` (no `validated` entry inside 180 days),
`memory-stem-collision` (two files with the same stem anywhere in one root),
`memory-unknown-key` (a frontmatter key outside the set above, which is what
catches `always:` and `owns:` coming back by habit), `memory-secret` (a
credential pattern anywhere in the file).

`memory-secret` is the one lint rule here with a paid-for incident behind it.
A live `lin_api_*` key reached at least 200 indexed chunks on this fleet, and
the fix was a redaction table applied before hashing, still live at
`packages/source/meta/src/sanitize.rs` (`lin_api_`, `gh*_`, `sk-`, `xox*`,
`AKIA`, JWT, PEM, `Authorization` headers). Reuse that table rather than
writing a second one. The exposure here is worse than it looks: `validated.how`
holds a command line, which is exactly the shape that leaked, and unlike a
transcript a memory is committed on purpose.

Two of these rules are evidenced and the rest are reasoned. `memory-body-budget`
and `memory-duplicate-tldr` match what the fleet measured as the real quality
levers, bloated bodies and duplicate hits ("the single biggest quality lever is
down-weighting or stripping tool_result bodies at ingest"; five identical shell
hits for one command). `memory-directory-budget`'s 150 is **not** evidenced:
the only study on this fleet is silent on corpus size, and the external claim
behind the number (2,400 records scoring 13% against 248 scoring 39%) is an
unlinked blog citation. Keep the cap as a forcing function for consolidation,
but do not defend it as measured.

`memory-body-budget` and `memory-unchecked` apply to `genre: memory` only.
A reference page is supposed to be long, and a 180-day validation clock on a
reference page produces a wall of errors that says nothing. That scoping is
what removes the need for an `evergreen` escape hatch, so there is no such
field: an exception you have to remember to set is a field that gets
forgotten.

There is no `owns:` field either, and dropping it is deliberate. `ix/docs`
declared 124 `owns:` globs per domain, read only by
`symphony-pack/skills/docsync.md`, which was deleted from ix on 2026-07-12
in 56f4e8d91e. So the globs have had zero consumers for 17 days. Carrying
them forward would mean maintaining metadata nothing reads, and a
staleness digest over them measures the wrong thing anyway: 499 of the last
2,155 commits under `crates/` added or deleted a file, so a membership
digest across 3,985 matched files would fire about 17 times a day while
saying nothing about whether a page is wrong. The conversion records every
dropped glob in its report, and git keeps them.

`memory-directory-budget` counts per leaf directory, not per root, so the
221-page docs conversion passes at 1 to 15 files per domain.

`--fix` handles only what is unambiguous: normalize whitespace, sort
`topic`/`handle`, refresh `based_on` hashes. It refuses to touch a file
whose frontmatter does not parse, the way `skill-lint` does.

## Elixir surface

`IxMcp.Memories`, aliased `Memories` in the workspace prelude. One
`Cmd.run` per call, no daemon, mirroring how `IxMcp.Memory` wraps `weave`.

```elixir
Memories.search("why did every host rebuild")          # ranked, [%Memories.Hit{}]
Memories.search("rebuild", topic: :nix, limit: 5,
                dirs: ["/abs/one/.memories", "/abs/two/.memories"])
Memories.roots()                                       # what the default resolves to
Memories.expand(hits, depth: 1)                        # add `related:` neighbours
Memories.show("nix-rebuild-cascade")
Memories.stale() / refuted() / unchecked(days: 180)
Memories.lint()

Memories.remember("nix-rebuild-cascade", "one-line tldr",
  body: markdown, topic: [:nix], handle: ~w(nix-dag), based_on: ["path/to.rs"])
Memories.validate("nix-rebuild-cascade", by: "claude-opus-4-6", how: "the command")
Memories.refute("nix-rebuild-cascade", by: "...", how: "...", instead: "new-slug")
```

`search/2` returns ranked structs. There is deliberately no `rank/1`: two
names for one result is how a caller ends up sorting twice.
