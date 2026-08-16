# eval-backend = shadow: what it is, and what it saw on the lang corpus

ENG-12546 part 3. Written so the numbers below can be reproduced and argued
with, rather than taken on trust.

## The short version: no unexplained value divergence, and one owner-approved one

261 corpus evaluations, every one compared. No crash, no unaccounted attempt,
and exactly three divergences about a *value*:

- `eval-okay-inherit-attr-pos` answers `[ null null null null ]` where cppnix
  answers positions.
- `eval-okay-getattrpos` and `eval-okay-getattrpos-functionargs` fail in the
  Rust arm with "expected a set but found null".

All three were the same decision: `builtins.unsafeGetAttrPos` returned `null`.
Nothing else disagreed about a value, and the Rust arm never produced an answer
where cppnix failed.

**Superseded 2026-08-06 (ENG-12137).** The IR carries positions now and all
three of those cases match cppnix byte for byte; their allowlist entries are
gone. The rows above are left as the measurement they were. What still answers
`null`, and why, is `maintainers/ix/positions.md`.

The other 73 divergences are all about failure *presentation*, which the parity
bar (CLAUDE.md) puts in tier 2. They are listed and grouped below because two
of the three groups have one systemic cause each, and both causes are worth
fixing.

## What shadow does

`eval-backend = shadow` evaluates with both backends and serves the C++ one.
The Rust arm runs after the C++ answer is known, with everything caught:

- A construct the Rust backend cannot evaluate is a `refused` verdict carrying
  the refusal token, not a user-visible failure.
- A disagreement is one `<4>`-prefixed line on stderr with a stable id, and a
  row in the `shadow` block of `NIX_SHOW_STATS`.
- A crash inside the Rust arm is caught and counted. So is a failure of the
  comparison machinery itself, which is otherwise the one bug that would
  silence exactly the runs that found something.

Turning it off is `eval-backend = cpp`, one setting, no second place to change.

Two commands are wired: `nix eval` with `--expr` or `--file`, and
`nix-instantiate --eval`. Everything else is counted as a skip rather than
refused, because under shadow the C++ arm answers it and nobody is denied
anything.

### The counted identity, and the hole it leaves visible

    agreed + agreed-failure + agreed-failure-text-differs
      + refused + mismatched + crashed  ==  attempts

`attempts` is incremented **before** the Rust arm is entered. An arm that dies
mid-call therefore leaves `attempts` ahead of the verdicts, and the difference
is printed as `unaccounted`. Incrementing on return would make that same death
indistinguishable from an evaluation that never happened.

Watched failing: with the `Agreed` verdict deleted, one trivial evaluation
reported `attempts 1 verdicts {} unaccounted 1`.

### Bounded overhead

Two guards, because "safe to leave on" is a claim about the worst case:

- A thread-local recursion guard. The Rust arm answers its own questions
  through hooks that run cppnix code (`rustCopyToStore` calls
  `copyPathToStore`), so anything down there that evaluated would otherwise be
  shadowed again from inside a shadow.
- `eval-shadow-budget`, seconds of Rust-arm time per process, default 120.
  A budget and not a per-call timeout, because there is nothing to interrupt a
  synchronous call into a static library with. What runs away on a command
  evaluating many attributes is the aggregate, and that is what this bounds.

Measured cost on the corpus: 0.35s of Rust arm across 261 evaluations.

That is a **cold-cache** number and an upper bound, not the steady state.
`eval-cache-dir` writes objects but never serves them on the handle path
(ENG-12830), and shadow evaluates through `rustEvalSelect`, which is that
path -- so the Rust arm recomputes from scratch on every invocation and gets no
memo hit it is entitled to. When ENG-12830 lands, the cost of leaving shadow on
should fall. Nothing about the comparison changes, only what the second arm
costs to run.

Stated rather than banked: a conservative number presented as the real one is
still a wrong number, and this one is going to move.

## The run

    maintainers/ix/shadow-corpus.sh build-shadow/src/nix

on a local macOS build of this branch, `-Dnix:rust-eval=enabled`,
nix-instantiate sha256 `98e34472...`. Full output in the PR; the summary:

    cases run:            261
    shadow attempts:      261
    rust arm time:        0.36s
    unaccounted:          0

    verdicts
      agreed                           125
      agreed-failure                    24
      agreed-failure-text-differs       49
      crashed                            0
      mismatched                        27
      refused                           36

    refusal tokens (rust arm, under shadow)
      unimplemented-builtin             15
      command-parser-lint                9
      rec-overrides                      4
      dynamic-attr-name                  2
      command-unsupported                1
      home-path                          1
      non-utf8-source                    1
      path-interpolation                 1
      unsupported-operator               1
      unsupported-syntax                 1

**Five of those ten names no longer exist.** `rec-overrides` (4),
`dynamic-attr-name` (2), `home-path` (1), `path-interpolation` (1) and
`command-parser-lint` (9) were retired by implementing the constructs, so the
tokens were deleted from `RefusalToken` along with their emission sites. The histogram above is left
as measured -- the rows sum to the `refused 36` line and editing them would
make the run's own totals disagree -- but nothing raises those five today, and
a census keyed on them reads zero because the key is gone, not because the
population is. Read this table as a record of one binary, not as the current
refusal inventory; `RefusalToken::ALL` is that.

Every row has a name. `unrecorded`, the sentinel for "this refusal crossed a
boundary carrying no kind", does not appear at all, and getting it to zero took
two fixes described below.

    divergence kinds
      value-mismatch                     1
      rust-failed-cpp-succeeded          2
      cpp-failed-rust-succeeded          0
      rust-crashed                       0
      error-class-lost                  29
      error-class-mismatch              24
      error-text-mismatch               20

### The two systemic presentation causes

**`error-class-lost` (29).** Identical message, different exception class. The
bridge maps a Rust status to a cppnix exception type and the mapping is coarser
than cppnix's hierarchy: `rustEvalThrow` sends status 1 to `EvalError`, so an
abort, a type error and a stack overflow all arrive as `EvalError` carrying
cppnix's own words. Its own histogram row precisely because it is not about the
evaluator; folded into `error-class-mismatch` it was 53 of 79 rows and buried
the four that are.

**`error-text-mismatch` (20).** Mostly cppnix's `assertEqValues` diagnostics
("string '\"x\"' is not equal to string '\"y\"'") where the Rust arm says
"assertion failed" (ENG-12138), and cppnix appending the offending value to a
type error where the Rust arm stops at the type.

## Two ways the census was losing names

Both were found by keying a histogram on tokens and then asking why a bucket
had no name. Neither would have shown up in a test of the evaluator, because
neither is in the evaluator.

**The session-less call had nowhere to put a token (ENG-12819).** `ixe_eval_expr`
renders a whole expression in one crossing and returns a status and a string,
with no session to hang a token on, so every refusal on that path reported
`unrecorded`. That path is not a corner: it is what `nix-instantiate --eval`
takes for a whole expression, and it is the memoised one. The handle API
carried its tokens correctly the whole time, so the census looked healthy from
`nix eval` and reported one unnamed bucket from nix-instantiate -- one arm green
and one arm blind, which reads exactly like two arms green. Measured before and
after on the same binary:

    before:  token=unrecorded            detail=builtins.filterSource
    after:   token=unimplemented-builtin detail=builtins.filterSource
             token=home-path             detail=~/... 
             token=rec-overrides         detail=rec { __overrides = ...; }

(`home-path` and `rec-overrides` have since been retired; both constructs
evaluate. The point the excerpt makes is about the token reaching the census
at all, which is unchanged -- reproduce it with any token still in
`RefusalToken::ALL`.)

Fixed by giving the call a token out-parameter rather than a second entry
point, because a legacy spelling that keeps losing tokens is the bug with a
longer name. Five tests hold it, including that a *non*-refusal reports no
token: without that one, an implementation that always wrote the last refusal
it saw would pass the others and mislabel every ordinary error.

**One refusal never had a token to lose.** Non-UTF-8 source is refused before
anything is compiled, and it was built as a bare message rather than a
`Refusal`, so it reached the census with no kind on either path. It now has
`non-utf8-source`, which is the last remaining row of the corpus histogram and
the reason `unrecorded` is now absent rather than merely rare.

## What this does not cover, stated plainly

- **`nix eval` cannot compare a failing evaluation.** `parseInstallables`
  evaluates the source on its way to building an installable, so a throw fires
  before `run(store, installable)` and before this command has an `EvalState`
  to hand the Rust arm. Those are counted as `cpp-failed-before-eval` rather
  than silently dropped. `nix-instantiate` has no such gap, and the 73
  both-arms-failed rows above all come from it.
- **One workload, and it is the language corpus.** These 261 cases are small
  expressions chosen to exercise language corners. A nixpkgs or ix-fleet
  evaluation would weight the refusal histogram completely differently and is
  the run that should decide the default flip. This is not that run.
- **macOS, one machine, one build.** Two corpus cases (`eval-okay-path-coerce`,
  `eval-okay-readFileType`) diverge only because `/tmp` is a symlink here and
  cppnix's accessor refuses a symlinked path while the Rust `Host` reads
  through it. Those two rows disappear when the corpus is addressed relatively,
  which is what the harness now does; the underlying accessor difference is
  real and belongs to ENG-12792.
### The Linux run: identical, and it found an id bug

Run on dev-compute-6 (Linux 7.1.4 x86_64) at the same revision as the macOS
run, both `4adca0d87`:

    cases 262, attempts 262, unaccounted 0, crashed 0
    agreed 127, agreed-failure 24, agreed-failure-text-differs 49,
    mismatched 27, refused 35
    value-mismatch 1, rust-failed-cpp-succeeded 2,
    cpp-failed-rust-succeeded 0, rust-crashed 0,
    error-class-lost 29, error-class-mismatch 24, error-text-mismatch 20

Every count identical to macOS, and the 76 distinct divergences are the same
kinds on the same cases. Only wall clock differs (0.58s of Rust arm on Linux,
0.73s on the Mac). So the histogram is a property of the evaluators and not of
the platform, and the earlier inference that the two `/tmp` rows were
macOS-specific is confirmed: they are absent from both once the corpus is
addressed relatively.

**What the comparison found is a defect in this report's own tooling.** The
divergence id is supposed to group one finding into one row across machines,
and it did not: it hashed the *absolute* path, so the same corpus divergence
was `bc45769e3203` from the macOS worktree and `03c08a51a0bb` from the Linux
checkout of the identical revision. A fleet query grouping by id would have
reported one finding as one row per host, which is the failure the stable-token
vocabulary exists to prevent, one layer up. The id now hashes the file's own
name and the attribute path; `maintainers/ix/shadow-id-portable.sh` holds it,
and was watched failing with the absolute path put back.

Residual limitation, which that gate's own first draft walked into: a
divergence whose *value* embeds an absolute path -- anything returning a
position, for instance -- still gets a per-machine id, and no amount of care in
the origin field fixes that. The id is stable for findings whose content is
machine-independent, which is most of them, and not for those.

- **No fleet run.** Two machines is not the fleet. Nothing here has run under
  a real workload on a production host, and the corpus is still 262 small
  expressions chosen to exercise language corners rather than anything ix
  evaluates.

  Partly answered since, in `maintainers/ix/shadow-fleet-run.md`: 2638 nixpkgs
  attributes against each of two trees, plus 215 ix fleet inventory
  attributes, on dev-compute-6. At `77b95143b` both nixpkgs arms evaluate
  100% with zero refusals, zero value divergences and zero tier-1 byte
  differences. It found three things this corpus could not, all three because
  the corpus has no large real tree in it. `stringLength`/`substring` reject a
  path cppnix coerces (ENG-12854, the one finding that blocks the flip). One
  interpolated path literal refuses the whole of the nixpkgs this fork pins
  (ENG-12852). And byte-truncating a divergence detail mid-UTF-8 destroys the
  whole process's `NIX_SHOW_STATS` census (ENG-12874), which silently
  undercounted that report by 7 divergences until its `stats files read` line
  caught it, and which is the "failure of the comparison machinery itself"
  this document names above.

  It also measured the overhead on a real workload at 5.9x wall clock over
  2638 attributes and 9.0x over 692, so the 0.35s over 261 tiny expressions
  quoted above was too small a workload to show the cost, and the ratio is not
  a constant to quote without its workload. That is the "this one is going to
  move" caveat coming true. It is still one host, and still not a production
  one.

## A cppnix bug this found on the way

`nix-instantiate --eval --strict` on `eval-okay-curpos.nix` answers
`[ 3 7 4 9 ]` when the file is named relatively and `[ 1 17 1 35 ]` when the
same file is named by an absolute path under a symlinked prefix (`/tmp` ->
`/private/tmp` on macOS). The second reading is the whole file treated as one
line. Both `eval-backend` arms reproduce it, so it is cppnix's position table
and not this backend, and `[ 3 7 4 9 ]` is what the corpus `.exp` expects.

The harness now runs from inside the corpus directory for this reason, which is
also what `lang-diff.sh` does.

## Reading the journal line

    <4>rust-eval shadow divergence kind=value-mismatch id=63ae64279f95 origin=<expr> cpp=5 rust=6

`<4>` so journald files it at warning priority; a line that merely says
"warning" in its body is invisible to every severity-filtered query, which is
the same reasoning `RefusalCensus::record` uses. `id` is a digest over the
kind, the origin and both truncated results, so one divergence groups into one
row across machines and runs. `origin` is the file and attribute path, which is
as precise as this backend can be: the Rust arm carries no source positions at
all (ENG-12714), so there is no line and column to report.

## Guards, each watched failing

- **A seeded wrong constant is caught.** `builtins.stringLength` made to return
  `len + 1`: `nix-instantiate --eval --strict -E 'builtins.stringLength
  "abcde"'` printed `5` to the user and
  `<4>rust-eval shadow divergence kind=value-mismatch id=63ae64279f95
  origin=<expr> cpp=5 rust=6` to stderr. Reverted.
- **An attempt with no verdict is visible.** Described above.
- **A vacuous corpus run is refused.** Not planned: the harness's own binary
  path stopped resolving once the loop cd'd into the corpus directory, every
  case failed, no stats were written, and the run reported zero divergences
  over zero attempts. Probe 2 refused it with "no evaluation was shadowed, so
  every zero below is vacuous". That is the guard doing the job it was written
  for, on a bug it was not written for.

## Three bugs this found in its own comparator

Recorded because each of them produced a confident wrong number first.

1. **`e.message()` on one arm and `e.what()` on the other.** `what()` on a
   `nix::Error` is the formatted error, prefix and ANSI colour included;
   `message()` is the text. Every failing case read as a divergence.
2. **Comparing coloured text.** nix colours messages whenever the stream looks
   like a terminal. Both sides now go through an escape stripper before
   comparison and before the report.
3. **Aggregation double counted the first sighting.** `setdefault` followed by
   an unconditional add reported every divergence as `x2`.
