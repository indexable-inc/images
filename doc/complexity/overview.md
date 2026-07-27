# complexity

`packages/complexity` ranks the units of a tree by how hard they are to hold
in your head, and gates the count of the worst ones so it can only go down.

## What it measures

The headline metric is **cognitive complexity** (Campbell, SonarSource 2018):
each break in linear flow costs one, plus the current nesting depth, so depth
compounds while breadth stays linear. A `match` costs one however many arms it
has; a run of like logical operators costs one however long the run; an `else`
costs one but carries no nesting penalty, because the reader already paid when
they read the `if`.

Each unit also reports **cyclomatic complexity** (decision points plus one, and
each short-circuit operator is a decision point), **max nesting depth**, and
**lines**. Cyclomatic is reported as a testability number, not a readability
one: an fMRI study of 19 programmers found it has no correlation with either
comprehension time or correctness (Peitek et al., ICSE 2021).

The whole output is triage, not measurement. The strongest published result on
cognitive complexity is that it correlates with comprehension time and
subjective ratings (Munoz Baron et al., ESEM 2020); the strongest counter is
that it is no better than the metrics it replaces (Lavazza et al., JSS 2023).
Low complexity reliably indicates readable code; high complexity does not
reliably indicate the reverse.

## What a unit is

| Language | Unit |
| --- | --- |
| Rust | `fn` items; closures are absorbed into the enclosing function |
| Python | `def`; lambdas are absorbed |
| TypeScript, JavaScript | function declarations and methods; arrow functions are absorbed |
| Go | function and method declarations; `func` literals are absorbed |
| Nix | attribute bindings, except those whose value is an attribute set |
| Elixir | `def`, `defp`, `defmacro`, `defmacrop` |

Two of these needed a decision nobody has published. Elixir has no keyword node
kinds at all: `def`, `case` and `if` are ordinary calls distinguished only by
the text of their target, and dispatch arms are `stab_clause` nodes. Nix has no
functions in the usual sense, so the unit is the binding; a binding whose value
is an attribute set is a namespace and is descended into, because reporting it
would absorb every member's score into one entry thousands of lines long.

A language with no profile yields no units rather than zeros, so an uncovered
corpus is visible in `stats.files_scanned` minus `stats.files_measured` rather
than silently diluting the budget.

The node kinds behind each profile are transcribed from `metric/src/dump.rs`,
which parses a sample of every construct and prints what the grammar produced.
A misspelled kind scores zero silently rather than failing, so it is not safe
to guess them; re-run the dump after a grammar bump.

## The gate

Two committed numbers in `complexity.toml` at the repo root.

**Thresholds** are per language and are this repo's own size-weighted 90th
percentile, derived by the method in Alves, Ypma and Visser (ICSM 2010): weight
each unit by its lines, sort by the metric, and read off the value where
cumulative weight crosses the chosen share. A threshold then means something
checkable, "the worst 10% of this repo by volume in that language", rather than
appealing to authority. The conventional numbers are not empirical: McCabe
called his 10 "reasonable, but not magical", and SonarSource reached 15 by
raising the number until the output felt less noisy.

Re-derive them with:

```
nix run .#complexity -- . --quantiles
```

**The budget** is `[budget] max_over_threshold`: how many units may sit at or
above their language's threshold. This is the ratchet. Lower it as units are
broken down; do not raise it without a written reason on the line above, and
keep roughly 10% headroom above the measured value, because a budget set flush
against its own measurement reddens main on ordinary drift.

Re-measure with:

```
nix run .#complexity -- . | jq .stats.over_threshold
```

The gate runs as the `complexity` stage of the repo lint (`lib/per-system.nix`),
so a breach fails the required `flake-check` status. `complexity .` walks up for
`complexity.toml`, prints the report as JSON on stdout, and writes a summary
naming the worst units to stderr.

## Output

JSON on stdout, always; `--pretty` indents it. The shape is
`{units, stats, budget}`, where `units` is truncated to `--top` (25 by default)
and `stats` counts every unit regardless. Human-readable progress and the gate
verdict go to stderr through `tracing`.

## Not built yet

- **Churn.** Change history is a stronger predictor than any static metric, and
  it costs one `git log` parse, but it cannot go in the gate: the lint
  derivation copies a tree with no `.git`. It belongs in the ranking behind a
  flag.
- **A changed-lines gate.** The sibling `clone` tool has one, running outside
  Nix in `.github/scripts/run-required-gate.sh` for the same reason.
- **Validation.** Nobody has read the top 25 and said how many are worth
  breaking down. That is the check that would tell you whether the ranking is
  any good, and it has not been done.
