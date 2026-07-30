# memories

`packages/memories` is a per-repo corpus of one-lesson markdown files plus the
CLI that searches, lints and writes them (`Cargo.toml:6`). A memory is YAML
frontmatter (`tldr`, `genre`, `topic`, `handle`, `prior`, `based_on`,
`validated`, `scope`) and a markdown body, one file per slug under
`<repo>/.memories/[<group>/]<slug>.md`; search is BM25 with per-field boosts
multiplied by
the author's confidence, the age since anyone last confirmed it, and how many
times it has held up. Library crate `memories` plus a `memories` binary
(`Cargo.toml:11-17`), `nix build .#memories`.

The file format, the score, the CLI flags and the JSON output are frozen in
`packages/memories/CONTRACT.md`, which the Elixir wrapper builds against as
well; see [kernel-surface.md](kernel-surface.md) for that side.

## Public surface (`src/lib.rs`)

- [`discover::load`] (`src/discover.rs:205`) reads every `*.md` in each
  `.memories` directory into a [`discover::Corpus`] (`src/discover.rs:95`):
  the parsed memories, the per-directory [`discover::Scan`]s, and the files that
  did **not** parse. A malformed file lands in `Corpus::failures` rather than
  aborting the load or disappearing.
- [`search::search`] (`src/search.rs:73`) ranks a corpus and returns
  [`search::Ranked`] rows carrying both `bm25` and `score`. There is no separate
  rank step: two names for one result is how a caller sorts twice.
- [`lint::lint`] (`src/lint.rs:79`) returns every [`lint::Diagnostic`], all of
  them errors.
- [`write`] (`src/write.rs`) is the only code that writes the format.
- [`report`] (`src/report.rs`) holds the JSON shapes, so the contract lives in
  one file.

## Discovery is a directory listing (`src/discover.rs`)

One level of grouping subdirectories and no more (`memory_paths`,
`src/discover.rs:296`). A group keeps a large corpus under the per-directory
budget and means nothing else: the slug is the file stem, never the path, so
`related:` and `show <slug>` stay location-independent wherever a file sits. A
file two levels down cannot be found by that walk, so instead of vanishing it is
collected and reported as a `memory-slug` diagnostic naming the depth. A corpus
that silently drops a file is the failure this format exists to avoid.

Default roots, nearest first: the `.memories` of the git toplevel of the cwd,
then of each enclosing git toplevel (submodule before superproject), then
`~/.memories` (`default_roots`, `src/discover.rs:156`). A toplevel is any
ancestor holding a `.git` entry, tested for existence rather than for a
directory so a worktree or submodule checkout (where `.git` is a file) counts.

`--dir` replaces that set entirely and repeats, which is what makes a test or a
one-off corpus reproducible. `Root::explicit` (`src/discover.rs:40`) accepts
either the repo directory or the `.memories` directory itself, resolving both to
the same corpus. An explicit root with no `.memories` directory is an error (the
caller named it); a discovered one is just a repo with no memories. A slug is
unique inside a directory (it is the file stem) but not across roots, so two
roots can each return the same slug, distinguished by `root` in the output.

An optional `topics.txt` beside the memories closes the topic set for that
directory; absent, any topic is allowed.

Every result reports the root set it read (`root_paths`, `src/report.rs:215`),
and `memories roots` prints the same set with no query. An empty result from a
root set that quietly resolved to one unexpected directory is indistinguishable
from an empty result from the right directories, and that is how a search tool
stops working with nobody noticing. Both surfaces emit the resolved set rather
than the subset that happened to exist, because two subtly different "roots"
arrays would be the same drift one step later.

## Parsing refuses to lose a file (`src/model.rs`)

Cursor's `.mdc` rules drop a file whose frontmatter is malformed and say
nothing. Here every failure is a [`model::ParseError`] carrying the lint rule it
belongs to and, where the YAML parser gives one, the file line: a missing fence,
an unterminated fence, empty frontmatter, a non-mapping document, an unknown key,
a value of the wrong type, an out-of-range `prior`, an unparseable
`validated.at`, a non-hex `blake3`, or a missing `tldr` (`parse_memory`,
`src/model.rs:301`). The three fence failures have three different messages
because they have three different fixes.

The frontmatter struct is `deny_unknown_fields` (`src/model.rs:252`): `topics:`
written for `topic:` is a memory that never matches the topic filter its author
set, so an unrecognized key is an error rather than an ignored line. `slug` is
accepted only so the linter can name it, since the slug is the file stem and the
format never writes it.

Two documented defaults and no others: `genre` absent is `memory`, `prior`
absent is 0.5. A key outside the format's set is `memory-unknown-key`
(`check_known_keys`, `src/model.rs:497`) rather than an ignored line, which is
what stops the retired `always:` and `owns:` coming back by habit. `scope` is
`shared` or `user:<name>`; there is no `always:` and nothing is ever injected
unasked, so a memory reaches a model only because something searched for it.

## Ranking (`src/rank.rs`)

```
score = bm25
      * (0.5 + 0.5 * prior)
      * genre_factor                            # historical | frozen 0.5, else 1.0
      * max(0.3, exp(-age_days / 90))           # age since the newest validated.at
      * (1 + 0.15 * ln(1 + n_ok))
```

A hit scoring below `MIN_SCORE` (`src/rank.rs:76`) is dropped rather than
returned: a query with no good answer comes back empty, so a caller can say
"nothing is written down about this" instead of acting on the least-bad match.
The value is deliberately low, and the reason is measured: BM25 is unnormalized,
and in the first three-memory corpus tried here a genuinely relevant hit scored
0.46 while a query matching nothing scored 0, so a floor of 0.5 emptied a small
corpus of its real answers. An absolute floor is the wrong shape for an
unnormalized score (a fraction of the top hit would not move with corpus size);
it stays absolute because the contract names a `MIN_SCORE`.

Each constant is a named `pub const` saying what it defends against and that it
is a placed guess, not a derived value (`src/rank.rs:16-57`). The two decisions
worth knowing: the age decay is floored at `AGE_FACTOR_FLOOR` (0.3) rather than
decaying to zero, because the harm from an old memory is reading it unflagged
rather than finding it; and a memory with no `validated` entry is not decayed at
all (`UNVALIDATED_AGE_FACTOR`), because decay measures time since the last
confirmation and a never-confirmed memory has no such interval. Reinforcement is
logarithmic so the tenth confirmation counts for less than the second:
confirmations of one fact are correlated, and a linear count would let a memory
validated in a loop dominate.

Field boosts (`tldr` 3.0, `handle` 3.0, `topic` 2.0, body 1.0) come from
`file_search::MultiFieldEphemeralSearch`
(`packages/file-search/src/ephemeral.rs:150`), the same `set_field_boost`
mechanism the on-disk index uses to rank a filename match above a content match.
The alternative, repeating a boosted field's text in one `content` field, needs
no new code but inflates the document length, and BM25's length normalization
then discounts every other term in that document.

Search asks the index for every candidate rather than `--limit`, because the
score reorders BM25 and truncating on BM25 first could drop the hit that ends up
ranked first (`src/search.rs:95`).

## Refuted versus stale

Deliberately different. A **refuted** memory (newest `validated` entry has
`ok: false`) is excluded from `search` unless `--all`, and listed by `refuted`.
A **stale** one (a `based_on` file no longer hashes to the recorded value) is
returned with `stale: true` and a `stale_reason` naming the path
(`src/stale.rs:29`). A glob `based_on` has no single content to hash, so it can
never be stale, but `memory-based-on-missing` still requires it to match
something.

Recorded hashes are compared over their common prefix
(`BASED_ON_HASH_HEX_CHARS`, `src/model.rs:55`): the format writes a truncated
digest because a 64-character one is unreadable in a diff, and this is edit
detection rather than an adversarial check.

## Writing (`src/write.rs`)

`remember`, `validate`, `refute` and `lint --fix` are the only writers.
`validate` and `refute` edit the existing text: the file is split into its exact
fence, YAML, and body pieces (`split_file`, `src/write.rs:293`), the new
`validated` entry is inserted after the last line of the existing block, and
every `based_on` hash is rewritten in place with its indentation and any trailing
comment preserved (`refresh_hashes`, `src/write.rs:360`). Everything outside the
lines being changed comes back byte for byte, including CRLF line endings, and a
memory's history is never rewritten, only appended to.

Three refusals worth knowing: `remember` over an existing slug is an error
rather than an overwrite, because that would drop the `validated` history, the
one part of a memory nobody can reconstruct; `remember` with a `--based-on` path
that matches nothing is an error, so a memory is never born failing its own lint;
and every writer refuses a file whose frontmatter does not parse, the same
refusal `skill-lint --fix` makes.

`refute --instead <slug>` writes to two files, because `supersedes` lives on the
successor in this format.

## Lint rules (`src/lint.rs`)

All errors; a warning nobody has to fix is a rule that should not exist yet, so
there is no severity field in the output. `memory-frontmatter`, `memory-tldr`
(missing, empty, or over 1024 chars), `memory-body-budget` (over 3000 estimated
tokens at 4 bytes per token), `memory-slug` (a non-kebab stem, a `slug` key in
the frontmatter, or a file buried deeper than one level),
`memory-topic-unknown`, `memory-related-unresolved`, `memory-duplicate-tldr`,
`memory-supersedes-unresolved`, `memory-based-on-missing`,
`memory-directory-budget` (over 150 files in one leaf directory),
`memory-unchecked` (nothing inside 180 days), `memory-stem-collision` (two files
in one root sharing a stem, which is one slug with two meanings),
`memory-unknown-key`, `memory-secret`.

`memory-body-budget` and `memory-unchecked` apply to `genre: memory` only. A
reference page is supposed to be long, and a validation clock on one produces a
wall of errors that says nothing; that scoping is what removes the need for an
`evergreen` escape hatch, which is why the format has no such field.

`memory-secret` runs the fleet's own redaction table
(`source_meta::sanitize::redact_secrets`) line by line (`check_secrets`,
`src/lint.rs:352`): if redaction changes a line, that line holds a credential.
Reusing the table rather than writing a second one means a pattern added there is
caught here too, and scanning line by line gives the diagnostic a line number
while a multi-line PEM block still trips on its `BEGIN` marker. The rule exists
because a live `lin_api_*` key once reached at least 200 indexed chunks on this
fleet; `validated.how` holds a command line, which is exactly the shape that
leaked, and unlike a transcript a memory is committed on purpose.

The `tldr` ceiling and the body budget reuse `skill-lint`'s values
(`packages/skill-lint/src/lint.rs:14,19`), which defend the same thing; the
directory and freshness budgets are placed guesses named in `src/lint.rs:21-33`.
The 150-file cap is explicitly not evidenced and its comment says so: keep it as
a forcing function for consolidation, do not defend it as measured.

Diagnostics are grouped by file and then by line, with whole-file findings after
the line-specific ones.

## CLI (`src/main.rs`)

`search`, `roots`, `show`, `stale`, `refuted`, `unchecked`, `lint`, `remember`,
`validate`, `refute`. Global `--dir` (repeatable) and `--json`; exit 0 on
success, 1 on a lint error or a slug that does not resolve, 2 on a usage error.

Two things go to stderr rather than into the JSON, because the contract's shapes
have no place for them and stdout must stay parseable: a file that did not parse
(`report_unparsed`, `src/main.rs:503`) and the case where no search root has a
`.memories` directory at all (`src/main.rs:488`), which would otherwise read
exactly like a query that matched nothing.

## Tests

`cargo test -p memories`: 63 unit tests next to the code they cover, and 20
end-to-end tests in `tests/cli.rs` that drive the real binary and assert the
JSON key set and key order, so a renamed key in the contract fails the build
rather than the Elixir wrapper.
