# What the guards caught on 2026-08-05, and what caught the guards

On 2026-08-05 three Opus critics read the fork's differential gates and seven
fixers changed them. The critics were looking for one class of defect: a gate
that passes while the thing it guards is broken. They found nine instances in
`maintainers/ix/` alone (ENG-12544), and the fixers found more while fixing
those.

This file records the exhibits worth remembering. Each section is headed by the
effect rather than the mechanism, so you can skim to the one you need. Every
number and every quoted line is from a real run; where a claim could not be
sourced from an artifact, it says so.

The shape common to all of them: **an assertion whose satisfied state is an
absence.** "No mismatches", "no error line", "nothing pending" are all satisfied
by a check that ran nothing. A gate in that state does not go quiet. It reports
success, often with a better-looking number than an honest run produces.

## A gate can report a perfect score having measured nothing

`nixpkgs-frontier.sh` compared its two evaluator arms with `cmp -s`. Two arms
that both produced nothing have equal exit codes and two empty stdouts, so that
was AGREE.

Turn the per-row timeout down until nothing can finish, and the pre-fix script
reports twelve out of twelve:

```
RESULT nixpkgs-frontier rows=12 agree=12 differ=0 refused=0
rc=0
```

Not one row ran. The same script after the fix, same conditions:

```
11 a small package set    TIMEOUT  cpp rc=124 rust rc=124 after 0.05s; this row measured nothing
12 package set attr count TIMEOUT  cpp rc=124 rust rc=124 after 0.05s; this row measured nothing
RESULT nixpkgs-frontier rows=12 agree=6 differ=0 refused=0 timeout=6 empty=0
FAILED: 6 row(s) timed out and 0 produced nothing on either arm. Those rows
        compared nothing; before this check they scored AGREE.
rc=1
```

The same hole with no timeout involved, from a row both backends reject:

```
OLD:  1 both arms reject it  AGREE                                     rc=0
NEW:  1 both arms reject it  EMPTY  both arms exited 1 and neither printed
                                    anything; cpp err=[error: … 'throw' …]  rc=1
```

Three earlier attempts at this break produced nothing, and the reason is worth
keeping. Shortening the row timeout to 1s cut off no row, because the heaviest
checked-in row is `attrNames` of the nixpkgs top level and `--strict` forces
only the names: 0.145s. A hand-written "slow" row using `builtins.length` also
finished instantly, because `length` never forces the list elements. A second
one was mangled through two layers of shell quoting and never ran at all.
Turning the timeout down to 0.05s so that nothing *can* finish is what finally
produced the 12/12 above.

## The person removing the bug class shipped an instance of it

`sigterm-gate.sh` was comparing total elapsed time against a 15s bound with the
signal sent at 5s, so an arm could take ten seconds to notice SIGTERM and pass.
The fix measures the gap between the signal and the process dying.

The first version of that fix built a Python f-string out of shell-quoted
numbers, which is a syntax error on Python 3.11. Every measurement came back as
the empty string. The comparisons, being `float('')`, raised, exited non-zero,
and were read as "the bound was not exceeded". The gate printed this and
reported success:

```
  rc=124 elapsed=s kill-delay=s err=[error: interrupted by the user ]
RESULT sigterm-gate rust_rc=124 rust_elapsed=s rust_kill_delay=s bound=10s signal_at=5s
SIGTERM CHECK PASSED
```

Nothing in the exit status said anything was wrong. It was caught by reading the
output and noticing `elapsed=s`.

This is the exhibit to keep. The author was hunting exactly this shape, on the
same day, in the file whose header warns about it, and still shipped it. The
guard-writing step is not immune to the thing the guard is for.

Two changes came out of it. Values go in as `argv` instead of being interpolated
into program text, and a measurement that is not a number is now its own
refusal:

```
BREAK  the timing helper returns nothing
  sigterm-gate: elapsed (rust arm) came out as '', which is not a number;
                the run measured nothing
rc=2
```

The real number, once it could be measured: 0.023s, 0.024s and 0.025s over three
consecutive runs, against a bound of 15s. A slack factor of 600. The bound is
now 2s, on the gap rather than on total elapsed.

## Half the guards one fixer wrote that day passed when first broken

Three of six, on PR #82. Reported by the author to the coordinating session; it
appears in no PR body:

> Six new guards, each broken deliberately. Three passed when first broken and
> had to be rewritten:
>
> - the membership test in `set_member_index` is vacuous today, so mutating it
>   changed no answer; the predicate is now split out and the test doctors the
>   member list.
> - the constants tripwire asked `constant()` what it returns, and an embedder
>   constant returns `None` in a test binary because nothing calls the setter, so
>   a second one left the list unchanged and it passed. Passing state was an
>   absence. The constants are now a declared table and the test reads the
>   declaration.
> - mutating `GetLocal` to push lazily proved nothing, since downstream ops force
>   anyway. The real invariant is `Slot::peek` returning `Some` only for a forced
>   value, so the mutation moved there.

Three separate reasons, and the middle one is the same defect the gates had: a
check whose satisfied state is an absence, written inside a tripwire meant to
catch absences.

The rewritten guards then paid for themselves within hours, during a rebase, with
no further work from the author:

> Two of those then earned their keep during the rebase without me doing
> anything: the constants guard fired when `builtins.currentSystem` arrived as a
> second embedder constant, and
> `compile_time_resolution_matches_the_slot_the_set_holds` silently absorbed
> `placeholder`, `addErrorContext`, `trace` and `warn` because it walks every
> member rather than a list I wrote down.

The second half of that is the design rule worth stealing. A guard that walks
every member absorbs new members for free. A guard that checks a list somebody
wrote down needs that person to remember, and they will not.

## A test can report a failure that has nothing to do with what it tests

While adding the tier split to `eval-allowlist.toml`, the new permission needed
a control test: an agent-approved `presentation-divergence` entry must be
*accepted*. A guard that rejects everything is as useless as one that accepts
everything.

The first version added a well-formed entry and checked `rc`. It returned
`rc=1`. That looked like a clean refutation and was worthless: the `rc` came
entirely from the pre-existing unapproved `eval-fail-eol-2` entry elsewhere in
the file, and said nothing at all about the entry under test.

Re-run isolated, with `eval-fail-eol-2` approved so that `rc` can only be
reporting the new entry:

```
rc=0
ids returned:
  eval-fail-eol-2
  eval-fail-string-nul-2
  eval-fail-fromTOML-overflow
  eval-fail-fromTOML-underflow
  eval-okay-made-up-presentation
PASS: the agent-approved tier-2 entry was accepted and is in the id set
```

Both versions are in the PR (#85) rather than the bad one being quietly
replaced.

## A comparison against a baseline that did not run makes everything look new

From the #94 rebase. Reported by the author to the coordinating session:

> my first comparison used the intermediate rebased commit as the baseline, which
> did not compile, so its clippy run produced nothing and made every warning look
> new. A clean-looking diff against a broken baseline is the same shape as the
> zero-rows problem.

A diff needs both sides to have run. When one side produces nothing, the diff is
not empty and it is not wrong in an obvious way: it is the whole of the other
side, presented as change. Check that the baseline produced output before
believing anything about what moved.

## A design defended by a wrong belief costs more than the design

From #94, adding a stable refusal token beside the prose of every refusal:

> The memoised result is persisted, and my first answer to that was wrong.
> `EvalResult` goes through the witness store, so I initially smuggled the token
> through the status string as `unimplemented:<token>` on the belief that
> widening a persisted struct would invalidate rows already on disk. Rebasing
> onto #91 showed that was false: the stored form is a canonical map keyed by
> name, and the `emissions` field #91 added tolerates its own absence for exactly
> this reason, with a comment saying so. So `token` is a field and there is no
> string to parse back.

The string-smuggling would have worked. What made it worth reverting is that the
argument for it was false, and a false premise in a comment outlives the person
who wrote it: the next reader inherits the belief rather than the check.

The same PR shows the useful follow-on distinction. A row written before the
token existed has no key at all, which is a different fact from a row that has a
key and is not a refusal. The first reads back as `Unrecorded`, a category
rather than a guess, so a census can report the size of the population it cannot
classify instead of counting it as "no refusal".

## Failing closed is not the same as failing legibly

A safe failure mode is still a failure mode, and "it errs on the safe side" is
how one gets left in place.

The ENG-12608 canary hands its scratch store a trust configuration. Two settings,
and they do not live in the same place: `require-sigs` is a *store* setting and
must ride the destination URI, because a store opened by URI takes its config
from that URI's params and a global `--option require-sigs true` never reaches
it. `trusted-public-keys` is not a store setting. Passed as a URI param, nix
says:

```
warning: unknown setting 'trusted-public-keys'
copying 1 paths...
error: cannot add path '/nix/store/8aqpw0r3...-ix-cache-canary-ia-...'
       because it lacks a signature by a trusted key
```

and carries on with an **empty** trust set. Every copy is then refused.

That is failing closed, and it is the worst available outcome short of failing
open. The refusal is indistinguishable from a genuine signature failure — same
error, same exit code — so the canary reddened on every healthy run while
appearing to have caught something. The warning that explains it is one line
above, at a severity nothing filters on, in the middle of copy output.

Two things worth taking from it. Where a setting can be silently ignored,
**verify the setting took**, not just that the operation refused: the difference
between "refused because the signature is bad" and "refused because I trust
nothing" is the whole measurement. And when a component accepts an unknown
setting with a warning rather than an error, that permissiveness is the defect
enabling this — the same shape as `Environment=` truncating a value at the first
space earlier the same day, which also produced a working-looking unit with half
a setting in it.

The general form: an error path that cannot distinguish *your input was wrong*
from *my configuration was wrong* will be read as the first, every time.

## A setting that cannot justify itself is a bug you have not found yet

From #91, on `eval-cache-dir`:

> **An interrupt was memoised.** A Ctrl-C arrives as an ordinary
> `EvalError::Eval` carrying cppnix's own wording, so the recorder could not tell
> it from an expression that genuinely fails, and stored it. With a cache
> directory set, that expression then answered `interrupted by the user` on every
> later run, for ever, out of a cache the operator had no reason to suspect.
>
> This one was found by the accounting gate below rather than by reading: the
> `ixe_set_interrupted` row had to state why the setting could not change an
> answer, and it could not.

The gate is an accounting table: every setter must have a row saying why that
setting cannot change an answer. The bug was found by a row that could not be
filled in, before anything ran. That is a cheap and reusable shape. Enumerate
the things that could break an invariant, require a written reason for each, and
the one nobody can justify is the defect.

## A gate passing does not cover a property it does not touch

#91 shipped `cache-semantics-gate.sh`, 265 files across six configurations,
cached against uncached, cold and warm. Its PR says "Every arm was watched
failing by reverting the fix it guards", and that is true.

The same change broke arm E of `rust-incremental-gate.sh`, which the six-config
gate does not touch. Measured on dev-compute-3 at `36ea3f8ff`, a clean checkout
of the tip with no local changes:

```
  round trip: match=143 skip=7 of 150 (skip budget 7)
  compared=150 agree=150 differ=0 want=150
  compared=300 agree=300 differ=0 served_from_memo=149 want=300
  compared=150 agree=150 differ=0 served_from_memo=143 compile_hits=143 want=150
  E FAILED: 0 hits in the last round, wanted 10;
            eviction is discarding entries that are still in use
RESULT: FAIL
rc=1
```

Arm E asserts that a size-capped store still serves. Filed as ENG-12601.

The first diagnosis in that ticket, mine, was wrong. I guessed the memo key,
because #91 put process globals into it and the rounds are separate processes.
That is a plausible story and it is not what happened. vm-sound-fixer found the
real cause, and it is worth more than the guess.

`store.rs` decides a witness is orphaned by looking for an object named after
the witness file:

```rust
// A witness is named by its module's object address, so it is an
// orphan exactly when that object is gone.
for (path, bytes, name) in entries(&self.witness_dir()) {
    if !self.objects_dir().join(&name).exists() && std::fs::remove_file(&path).is_ok() {
```

#91 changed the witness filename from the module hash to an `EvalId`, which is
the module hash combined with a settings fingerprint. The object is still named
by the module hash alone. So `objects_dir().join(&name)` finds nothing for every
witness, and every sweep deletes every witness. Hits went from 10 to 0 on a
size move of 2.5%.

Two lessons, and both are ones this document already has in a weaker form.

The comment quoted above is `store.rs` asserting an invariant that `readset.rs`
owns. That is the doc-claim sweep's finding except load-bearing at runtime: the
sweep does not merely describe the naming rule, it depends on it, and nothing
broke when the other file changed the rule. A comment that states another
module's behaviour is a dependency with no compiler behind it.

And the six-configuration gate missed it because **it never caps a store**. Six
configurations sounds like coverage and is coverage of one axis, the settings
axis, repeated six times. Eviction is a different axis, and no number of
configurations along the first one reaches it. Counting configurations is not
counting properties.

The procedural half is worth keeping too. The failure first appeared on a feature
branch, and the branch author rebuilt a clean checkout of the plain tip before
reporting it, so the attribution would be evidence rather than assumption.
Without that step the report is "my branch is red", which is the wrong lane and
the wrong owner. It also, as it turned out, would have been the wrong cause: the
same discipline that got the lane right did not stop the guess about why, and
only somebody reading the sweep found that.

## A DIFFER where a REFUSED used to be is not evidence about that refusal

From #93, verbatim:

> With `unsafeGetAttrPos` answering `null` (#89), `hello.outPath` went from
> REFUSED to DIFFER with `expected an integer or float but found a set`.
> `hello.src` is a `fetchurl`, so that reads as fixed-output derivations. What
> separated them was checking the refusal directly: a minimal fixed-output
> derivation refuses cleanly by name, so the arithmetic error had to be something
> else. **A DIFFER appearing where a REFUSED used to be is not evidence about
> that refusal.**

Removing a refusal reveals whatever was behind it, which is usually not the
thing the refusal named. The check that settles it is to reproduce the refusal
on its own, minimally, away from the case that surfaced it.

## Where a guard lives decides which evaluations can see it

From ix#9931, on a detector that checks every health-metric file has a writer.
The break was to narrow the detector to one file, and it fired against a host
toplevel eval and not against the cheaper thing the author tried first:

> The last one is worth calling out: it did **not** fire against `nix eval` of a
> check's `drvPath`, because the assertion hangs off `healthDir` and nothing in
> that closure forces it. It fires against a host toplevel eval, which is what CI
> and the deploy do. A guard I had not watched fail would have looked fine.

Nix is lazy, so an assertion is only reached by an evaluation that forces the
attribute it hangs off. A break test run against a cheaper evaluation surface
than the one the guard protects proves nothing, and proves it silently: the
break runs, the assertion is never reached, and the result is indistinguishable
from a guard that fired and found nothing wrong.

The rule that falls out: run the break against the evaluation surface the guard
is meant to protect. For a fleet assertion that is the host toplevel, which is
what CI and the deploy evaluate, not a single check's `drvPath`.

## A gate that compares two artifacts in the repo cannot see the one in production

Every other hole in this document is a gate that could not see its subject. This
one is a gate whose subject was the wrong artifact, and it is the sharper shape
because the gate was working perfectly.

`nix/checks/cross-artifact/row-schema.py` checks that every `#[derive(Row)]`
struct agrees with the committed `*-schema.sql`. ENG-12608 added a column to
`metrics.cache_canary_runs`, in both places, correctly. The gate went green and
was right to: the DDL and the struct agreed exactly.

Then every canary tick wrote nothing at all:

```
ix-cache-canary: the canary could not reach ClickHouse, so this run is unrecorded:
  DB::Exception: Unknown expression identifier `input_addressed_out_path` ...
  (UNKNOWN_IDENTIFIER)
```

`CREATE TABLE IF NOT EXISTS` is a no-op on a table that already exists. The
committed DDL described the table the schema file *would* create; the live table
had been created a few hours earlier from the previous version of that same file
and never grew the column. Both halves of the canary failed on every run, for
both roles, and the only symptom was a `failed_to_run` log line.

The fix is one `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` beside the `CREATE`,
which the fleet's older schema files already carry with the reason written out
(`init-metrics-schema.sql:53-55`). The lesson is not "remember the ALTER". It is
that **a two-artifact comparison silently scopes itself to the two artifacts**,
and neither of them is the running system. Nothing in the check's name, output
or design says "this agreement does not imply the deployed table matches", and a
reader has every reason to think a passing schema gate means the inserts will
work.

The same shape is worth looking for wherever a check compares a generated file
to its generator, a lockfile to its manifest, or a schema to a struct. The
question that finds it: *if production had drifted from both of these, would
this check say anything?*

## What changed in the repo

`maintainers/ix/gate-ratchets.sh` is new and holds every expected count in one
sourced file. Exact where the input is a checked-in list, a ratchet where it is
the lang corpus. The old floors were `served > 30` and `produced > 20` against
observed values of 51 and 27, so a third of the coverage could disappear
silently.

It also records the revision and host its numbers were measured at, and every
gate prints that on its RESULT line:

```
RESULT cases=60 match=40 mismatch=0 unimplemented=3 both-fail-alike=17 \
  expected-cases=60 min-match=40 ratchets-from=15b970ef4@dev-compute-3
```

That field exists because 48 merges landed on `ix-patched` that day. One branch
carrying ratchet values was rebased eight times in an afternoon, and several of
those rebases moved the numbers. A ratchet minted against a stale tip is wrong
in one of two directions: too low and a regression passes, too high and the
next merge is blocked by a number nobody can reproduce. Rebasing onto the search-path work moved three values at
once (`match` 118 to 119, `unimplemented` 46 to 45, round-trip skips 9 to 7),
and every gate would still have passed on the old numbers, hiding the
improvement.

`eval-allowlist.toml` is now split by parity tier and parsed rather than
grepped. Tier 1, anything feeding a hash, has no representation in the file at
all and the gates that compare hashes do not read it. Tier 2, presentation and
error wording, is agent-approvable with a reason of at least 40 characters.
`semantic-divergence` needs a human named in
`maintainers/ix/eval-allowlist-approvers.txt`.

`lang-diff.sh` reached `mismatch=0` for the first time. The trajectory, all
measured on dev nodes:

| point | mismatch |
|---|---|
| `855e9b4d5`, morning baseline | 2 |
| after the `.flags` fix (ENG-12438) exposed three hidden divergences | 5 |
| after `eval-okay-callable-attrs` and `eval-okay-curpos` were fixed upstream | 3 |
| after the literal-lint refusal (ENG-12569) | 0 |

The jump from 2 to 5 is the interesting one. Four corpus cases carry a `.flags`
file, and the eval-fail loop let those flags *replace* the default flags instead
of adding to them, so the cases ran without `--eval` and scored `unimplemented`
for a reason unrelated to what they test. Three of the four turned out to be
real divergences that the bucket had been absorbing.

## What is not covered

- Three exhibits are quoted from agent reports to the coordinating session
  rather than from a PR body or a commit: the six-guards section, the
  broken-baseline clippy section, and the follow-on about the constants guard
  firing during a rebase. They were reported in a chat channel that leaves no
  artifact, so they cannot be re-derived from the repository. They are marked
  where they appear. Everything else here is quoted from a PR body or from
  output of a run named with its host and revision.
- No count of how often the fixed gates now fire in anger. Every guard here
  was watched failing under a deliberate break. None has yet caught a defect
  nobody knew about, except arm E, which caught ENG-12601 within hours.
- `nixpkgs-frontier.sh`'s TIMEOUT branch was verified only by lowering the
  timeout to 0.05s. No naturally slow row exercises it, so the branch is
  correct under a synthetic condition and unproven under a real one.
- The two-tier allowlist rule is enforced only for `lang-diff.sh`. Nothing
  mechanically prevents a future gate that compares hashes from being taught to
  read the allowlist. The prohibition is a comment in
  `maintainers/ix/eval-allowlist.toml` and in `drv-parity.sh`'s header.
- The three cache-canary exhibits (the wrong-artifact schema gate, the control
  that caught a broken freshness check, and failing closed illegibly) were all
  produced on dev-compute-1 and dev-compute-6 against a local HTTP binary cache
  with a test signing key, because dev hosts hold no credential for
  `cache-internal.ix.dev`. The mechanisms are the production ones -- a real
  daemon write-through, the fleet's ClickHouse -- but the production endpoint
  and the real fleet key are exercised by the deploy, not by these runs.
- Nothing here covers ix CI in general. These are mostly the fork's gates.
  Exhibit 8 is the one ix item, and the ix side of the day is recorded
  separately.

The scope gaps the day leaves open, which matter more than the documentation
gaps above:

- The bulk of `cache-push-tools.nix` is unreviewed.
  `nix/modules/services/host/ci-dispatcher/cache-push-tools.nix` in the ix repo
  is about 4,500 lines (4,485 at `1197696318f`). ix#9931 changed and guarded
  part of it. The remainder has not been read against the vacuous-pass shapes,
  and it is the file that decides what reaches the binary cache.
- The broker admission surface was not audited. Nothing in the day's work
  looked at what the broker accepts, so the shapes found in the executor and the
  publish leg may exist there too. No claim is made either way; nobody looked.
- ENG-12597: five cases were ceded on purpose. The literal-lint refusal keys
  on the setting rather than on the lint firing, so five `eval-okay` cases that
  cppnix accepts are refused too. `LANG_DIFF_MIN_MATCH` went from 123 down to
  118 to accommodate that, which is a floor moving the wrong way, recorded in
  `maintainers/ix/gate-ratchets.sh` beside the number.
- Arm E is diagnosed and assigned, not fixed. ENG-12601 has its root cause
  (the witness-orphan sweep, above) and an owner. Until it lands,
  `rust-incremental-gate.sh` is red on `ix-patched` and every capped store
  loses its witnesses on the first sweep.

## The rule this is all one instance of

A guard you have not watched fail is not a guard. The corollary the day added:
watching it fail is not enough either, because the break itself can be wrong.

Two fixers hit this independently and counted it. One wrote six guards and three
of them passed when first broken, for three different reasons. Another wrote
breaks against the gate scripts, and three of those passed on the first attempt
for reasons that had nothing to do with the guard. Each would have been recorded
as "watched failing" by an author in a hurry. The gate-script ones:

- a 1s timeout that cut off no row, because the rows are 0.145s
- a `builtins.length` workload that forces nothing
- `eval-backend = rust` pinned on a rollback check that still returns `42`,
  because the Rust backend also computes 42, which is exactly why that check
  asserts the evaluator and not the value

A third fixer hit it on ix#9931, twice. Once by running the break against
`nix eval` of a `drvPath`, which never forces the attribute the assertion hangs
off. Once at the wrong scale: `printf '%s\n' "$copies" | grep -qF` takes SIGPIPE
when the producer blocks, and `pipefail` reports 141 as the assertion failing.
That is invisible below one pipe buffer, so the small scenarios passed and the
2,046-derivation one did not. A break that only fires past a buffer boundary is
one most fixtures are too small to run. Filed as ENG-12599.

Counting them: at least nine first attempts that day did not test what their
author thought. Three because the guard itself was vacuous, three because the
break did not exercise it, and three because the harness around the break
misreported. All nine were found by the author, before review, by looking at the
output rather than the exit code.

When a break does not fire, the first hypothesis should be that the break is
wrong, not that the guard is redundant.

### Before believing a zero, check the denominator

The third of those groups is worth separating, because it is the one a careful
author still walks into, and because the rule that catches it is cheaper than
the care.

Three of the day's measurements produced empty output. A clippy run whose
baseline commit did not compile. A producer killed by SIGPIPE before it wrote
anything. A journal query pointed at a bus the units were not writing to. In
every case the failing state and the passing state produced **identical**
output: "nothing here" in exactly the words of "nothing wrong".

The last of the three is the clearest, written up by its own author in
`maintainers/ix/eng-12546-refusal-census-handoff.md`. That file arrives with the
ENG-12546 census work; until it lands the text is at commit `a4c668e98`:

> The first run of that pair returned **empty output from both queries**. The
> tempting reading is "no priority difference". The correct one is "I do not know
> whether anything ran" -- and it was the second: `systemd-run --user` returned
> rc 0 while writing to a journal the query was not reading, so the `||` fallback
> to the system bus never fired and both sides produced nothing.
>
> An empty result and a negative result look identical and mean opposite things.

Chasing it produced the real evidence: `PRIORITY=4` visible at `-p warning` was
1, and unprefixed `PRIORITY=6` visible at the same filter was 0. That is a
measurement. Two empty queries were not.

No amount of vigilance distinguishes those two states, because there is nothing
to see. What distinguishes them is a second question asked before the first
answer is believed:

- Did the thing run at all? Not "did it exit 0", which `systemd-run --user` did
  while writing nowhere useful, and which a non-compiling baseline also manages.
- Did the query have any rows to match against? Count the population carrying
  the join key before reporting how many matched.
- Is the count of the thing being waited for greater than zero? "Nothing is
  pending" and "all four finished" differ exactly when the set can change under
  you.

A zero deserves more suspicion than any other number, because a query that
matched nothing looks exactly like a query that found nothing. Every gate hole
in this document is a special case of that, and so is every wrong break.

#### The same failure with a control attached, and the control is what caught it

The three instances above were all found the hard way, by chasing a zero until
it explained itself. A fourth from the same day shows what the cheap version
looks like, because the author walked into it twice and then got out with one
extra line.

Validating the ENG-12608 canary needed proof that the binary about to be tested
was built from the current source. The check grepped the ELF for strings unique
to the new code:

```
markers: transition=0 ia=0
STALE_REFUSING
```

Read as "the build did not pick up my changes". Two rebuilds were launched on
that basis. The source on the host had the changes the whole time. The check
called `strings`, which is not installed on dev-compute-1, so every invocation
failed and every count came back 0 — a failed command and an absent string are
the same empty output.

What broke the loop was grepping for a string *known* to be in the old code:

```
-- strings sanity: a string we KNOW is in the old code --
0
```

`narinfo_signature` is unquestionably in that binary. A check reporting it absent
is a broken check, and that reading takes no insight — it is a fixed
counterexample the check must produce a nonzero answer for. Rewritten with `grep
-a` and the control asserted first:

```
control narinfo_signature = 7
  not_contracted_but_present     1
  require-sigs=true              1
FRESH
```

The check now exits without a verdict when the control comes back empty, and it
did exactly that on the next run, against a `libnixstore.so` picked by a glob
rather than from the deployed closure. That refusal was correct and would
otherwise have been an authoritative "fix absent" about the wrong file.

This is the denominator rule with a control attached, and the control is the
stronger half. "Check whether the thing ran" requires noticing that you should;
a control fails loudly on its own, every run, with no vigilance required. Any
check whose passing state is *absence of a match* can carry one for a line or
two: assert a fixed input the check must find, and refuse to answer when it
cannot.

#### One run's silence is not a verdict, in either direction

The same mistake appeared twice on 2026-08-05, four hours apart, pointing in
opposite directions, and the second time it was made by the agent who had used
it to correct the first.

At about 11:00Z, `jj views` failed on PR ix#9932 and did not appear in main run
`31010873307`. The coordinating session read the absence as evidence and told
its author to deprioritise ENG-12605 as PR-context-specific. It was not: `jj
views` failed on main at run `31016230910`, on the merge commit `3d71d2e69ad`.

At about 15:40Z, that same author looked at ENG-12631 -- the signing key missing
from `/run/ix-nix-cache-daemon/` -- saw it absent from main run `31018027778`,
and wrote "transient or already addressed". It was neither. Another session had
root-caused it and remediated both affected hosts at 15:02, before that run
started, and the underlying defect recurs on any activation that restarts the
materializer alongside the dispatcher.

The denominator of "did not appear in this run" is how many runs could have
shown it. Both times, one. That is not a sample.

The two directions need different corrections, which is why they are two
paragraphs rather than one rule:

- *Absent, therefore gone* is answered by asking whether the run could have
  reached the check at all. Through most of that day it could not: runs died at
  worker setup, at the publish wall, or at an IFD eval that never produced a
  verdict table. A check that never ran and a check that passed are the same
  colour of silence.
- *Was failing, now absent, therefore transient* is answered by asking who fixed
  it. This is the more expensive error, because "transient" is the one word that
  strips a ticket's priority without closing it. A fixed thing read as transient
  loses the follow-up that makes the fix permanent -- here, the fix that stops
  the next activation reintroducing it.

Neither author of this section is neutral about it; both are subjects of the
exhibit. That seems closer to the point than a problem with it.

## Correct code, and tests asserting the one output the mistake does not move

Everything above is a guard that could not see, a break that did not exercise,
or a harness that misreported. This is a different shape. The implementation is
right, the tests pass, and the tests could not have failed. Four instances
landed on 2026-08-05, in four different builtins.

The honest one is mine, because it is the same author writing both halves. I
implemented `builtins.toFile` and had its result return a bare string with no
context. I also wrote the test, and my test asserted the returned path, which is
byte-identical whether or not the context is there. I found it by running
`builtins.getContext` against the cpp binary and seeing it disagree:

```
{ "/nix/store/m6wswa7yn6x5gi6gdq7x1fqlwmlhfja9-hello.txt" = { path = true; }; }
```

That is the pattern, and it is not carelessness. The natural thing to assert is
what the expression returns. Every one of these bugs lives in what something
downstream computes from it.

### The four

**The `r:` prefix in the ATerm (#108).** `outPath` is built by
`make_fixed_output_path` from `render_prefix`; the `.drv` carries
`print_method_algo`, a second call site for the same prefix, and that string is
also the `<methodAlgo>` field of `hashDerivationModulo`'s `fixed:out:` payload
that every dependent's output path is built on. Dropping the `r:` from the
second one passed the entire suite. Measured with that one edit applied:

```
                      correct                           r: dropped
 source .drv   7nw8jhad9wcsrki6m9gapv8hxcy8vhpx  1iy3iqa8bybc3mdhx05lyppzcvd19cak
 source out    7q2y7crif31i14hipkifg4w8n05zahdd  7q2y7crif31i14hipkifg4w8n05zahdd
 dependent out rzl2ij7libwahjhay8wnqq5vkca2cm2i  3akdijx07x8is1sw5972qgrvj0arbx9q
```

The middle row is why nothing saw it: every fixed-output test asserted
`.outPath`. The bottom row is the cost, which in nixpkgs is every `fetchzip`
and `fetchFromGitHub`.

**`toFile`'s reference list (#112).** Deleting `references.push` left the suite
green. Every existing case passed contents with no context, so the reference
list was always empty and nothing observed it. `makeType` embeds each reference
in the store path's type string, so a dropped one is a well-formed path for a
different store object.

**`toJSON` of a path (#110).** vm-bughunt applied #108's rule to its own merged
code and found the same shape: "My merged tests asserted what the expression
returns and that the result has *a* context. Neither would have caught the copy
failing to become a dependency."

That one also records the technique, which is the part worth stealing. The
obvious test does not work, because the JSON string is an environment variable
and its bytes move the `.drv` whether or not the context propagated.
`unsafeDiscardStringContext` gives the same bytes with no context, so two
derivations differ in `inputSrcs` and in nothing else and must not land on the
same `.drv`.

**`toFile`'s result context**, mine, described above. Worth one more line
because it is the only one where the same person wrote the bug and the blind
test: cpp's own comment says `prim_toFile` does not add `context`, which reads
as "the result has no context" and means "not the *input's* context". The value
still goes through `mkStorePathString`, which attaches an `Opaque` naming the
path just written.

### The rule

For any value that feeds a hash, assert what a consumer computes, not what the
expression returns. A store path, a context element and an ATerm field are all
inputs to something else, and the something else is where a mistake becomes
visible. Where a golden is unavailable because the test has a fake store, a
relational assertion works: two spellings that must differ, or must not.

A second rule falls out of the first instance. A default that renders as the
empty string hides a whole class of prefix errors. `flat` ingestion has prefix
`""`, so a flat-heavy golden set exercises the prefix code path and observes
nothing about it. When a formatter has an empty-string case, the tests have to
include a non-empty one deliberately, because the empty one passes either way.

### Two checks that came back clean, and one that cannot be made

A class supported only by its confirmations is the shape this document keeps
refusing, so the negative results belong here too. Three properties of the
merged `toFile` were checked by breaking the code rather than reading it. One
was the reference list above. The other two were clean.

The result's context on the merged implementation is correct. Removing the
`Opaque` element from the `StoreText` driver arm fires
`to_file_asks_the_store_and_returns_its_answer`. The mistake I made on my own
copy was not made on the one that shipped.

The `readset` encoder split is correct, and this is the one I most expected to
find. `question_value` enumerates the parts beyond the argument by hand, and
`Question::key_parts` enumerates them again for the digest, so the two can
disagree, and a variant that keys right and encodes wrong can never cache-hit.
That is the ENG-12443 shape, and my own `AddTextToStore` hit it.
`Question::StoreText` does not: it has a tag, a `key_parts` arm, a `question_value` arm, a decoder
arm, and a two-reference sample in `one_of_each`. The structure is still a trap
for the next variant and it is a guarded trap. Deleting the `question_value`
arm, which is exactly the shape mine had, fires two named tests:

```
readset::tests::a_witness_naming_every_question_reads_back
readset::tests::every_question_variant_round_trips_through_the_witness_codec
```

So the verdict is cleanup at most, and it is recorded rather than filed.

The third is a negative result the method cannot turn positive.
`references.sort()` in `toFile` cannot change anything: the context is a
`BTreeSet<ContextElem>`, `ContextElem` derives `Ord` with `Opaque` first, so
all-opaque elements already iterate in path order. Removing the sort leaves
every test green and no test can distinguish it, because the ordering is
structural. It is dead and harmless and it stays. Reporting it as a third find
would have been the easiest wrong thing to do all day.

### The moment a check was taken is part of the check

Three of the day's duplicated implementations came from a presence check that
was correct when it ran. Mine read `origin/ix-patched` at `b17945c0e` with real
denominators, counting lines in each file before counting matches, and found no
`toFile`. `b17945c0e` was my own merge and an ancestor of `d066bdbfc`, which
added `toFile` about ten minutes later, while I was building on a dev node.

Fetching more often is not the habit that saves this. The window that matters is
between starting and opening the pull request, and it is exactly as long as a
dev-node build. Re-check presence immediately before pushing. This is the same
lesson as the day's other timing bugs: a check is only as good as the moment it
was taken, and a stale input produces a confident wrong answer.

## This document carried the bug it is about

For about forty minutes on the day it was written, the arm E section of this
file stated a root cause that was wrong. I had guessed the memo key, because
#91 put process globals into it and the eviction rounds are separate processes.
The guess fit every fact I had and was not the cause. It went into ENG-12601 and
then into this document, and both were read by other people before either was
corrected.

The failure was not the guess. Guessing is how a diagnosis starts, and a wrong
first hypothesis costs nothing if it is labelled as one. The failure was
publishing it without the check that would have settled it, which was reading
`store.rs`'s sweep, and which took one person a few minutes once they did it.
The fix took one PR.

That is the cheapest available demonstration of everything above. A conclusion
that has not been checked looks exactly like one that has, from the outside and
in the writing, right up until somebody looks at the source. The rule this
document is about does not stop applying to the document.

## An optimisation that skips work deletes the tests that measured it

Everything above is a guard that could not see, a break that did not exercise, a
harness that misreported, or code whose tests assert the one output the mistake
cannot move. This is a fifth shape, and the one the day's opening frame does not
cover. The assertion was **not** written vacuously. It was made vacuous later,
by a change somewhere else, and a test that had been measuring the right thing
went on passing while measuring nothing.

ix#9846 (`d953f55dc8e` / `c3f8615d16b`, 2026-08-04T10:25Z) added a warm gate to
`drain_group` in `cache-push-tools.nix`:

```bash
elif ! { roots_present_remotely "${targets[@]}" || bulk_attic_push; }; then
```

`roots_present_remotely` is a non-recursive `nix path-info --store <substituter>
--json`: skip the bulk upload for a group whose roots the cache already holds.
It is correct, and its comment prices both error directions honestly — nothing
releases on the gate's word, because `prove_published_closure` remains the sole
release authority. There is no bug in the production code.

The fixture's fake `nix` has no model of what the substituter holds. Any
`--store` query with no failure flag set exits 0. So `roots_present_remotely`
succeeded every time, `bulk_attic_push` became unreachable in **every**
scenario, and `nix/checks/ci-cache-push.nix` — 3,938 lines of it — stopped
exercising the push path at all.

Most of it kept passing, because most of it asserts things that stay true when
no push happens: the roots still release, the queue still drains, the proof
still succeeds. Here is the drain's own summary, immediately before the
failure:

```
ix-cache-push-drain: published and verified c2h2f4cw9p8i8zcfy52fd1dd6g0yhnki-hello-2.12.3
ix-cache-push-drain: published and verified xj9dgyqrcq8hrf4mrkvbcp4pa3hgrbhy-gnugrep-3.12
ix-cache-push-drain: published and verified mp8s10fwm685azvvv1qq7zyf7iajjlj8-coreutils-9.11
ix-cache-push-drain: tick summary: selected 3, released 3, quarantined 0, pending 0 at tick start
ci-cache-push: assertion failed at builder line 2176:
  test "$(grep -c '^push ' /build/ix-cache-push-commands)" -eq 2
```

Three paths published and verified, three released, zero pushed. The only
assertion that noticed was the first one that *counts pushes* — four closure
paths at `atticPlanBatchSize = 2` must be two `attic push` invocations, which is
the bound on Attic's eager `PushPlan` metadata fan-out. Everything ahead of it
in the script passed.

It merged on 08-04 and was found on 08-05. It survived the day because main's
aggregate `ci` conclusion was already `failure` before it landed, so the new red
was indistinguishable from the old. ENG-12611.

The fix is entirely in the fixture, which had to be able to say "the cache is
cold" and had to **default** to cold, since every other scenario needs the push
path to be the path it measures. That needed no new production surface: the warm
gate is the only remote `path-info --json` in the tool set *without*
`--recursive`. All ten `path-info` call sites in `cache-push-tools.nix` were
read rather than assumed — the closure proof (`:2658`) and the realisation batch
(`:1438`) are recursive, the hit-rate probe (`:3772`) carries no `--json`, and
`nixCacheQueryArgs` (`:1034`) is `--option http-connections N` with no `--store`
at all.

The coverage ix#9846 shipped without asserts both halves: that the push is
skipped, **and** that the roots still release behind it. One that checked only
"no push" would pass just as loudly if the gate had broken release instead.

### The rule

A short-circuit's success condition is that expected work did not happen. Every
test that measured that work is now measuring its absence instead — and where
the fixture cannot express the new precondition, those tests do not go red. They
go vacuous, and stay green.

So the review question for any cache hit, early return, dedup, skip-if-present
or fast path:

> **Which existing tests measured the work this now skips, and can the fixture
> still turn the skip off?**

If the second half is no, the change has silently deleted the first half's
coverage, and it needs a fixture knob before it needs anything else. ix#9846
changed exactly one file and added no test of the gate. That is the shape to
catch in review, and it is cheap to catch, because the diff that introduces it
is usually small and reads like a pure win.

The honest limit: this rule is one measured instance wide. It is written
generally because the mechanism is not specific to caches — anything that makes
work conditional has it — but only the cache-push case was actually observed.

## A loud bug is a place to look for a quiet one

`builtins.concatStringsSep` refused an attribute set. That was the reported
failure (ENG-12628): evaluation stopped with `cannot coerce a set to a
string` on two of the first three nixpkgs `drvPath` rows sampled -- three rows
is a hint about the rate and not a measurement of it. Loud, diagnosable, and
fixed in an afternoon.

The function had two more defects, and the fix only found them because the
question asked was "what does cppnix do here?" rather than "why does the set
fail?".

The second one does not fail. A path element returned the **source** path:

    builtins.concatStringsSep ":" [ /m/a /m/b ]
    was:  "/m/a:/m/b"
    cpp:  "/nix/store/h-m-a:/nix/store/h-m-b"

cppnix coerces with `copyToStore` at its default of on, so the tree is copied
into the store and the string carries the store path plus the context that
records the dependency. The old code produced a plausible string with no
context, which reaches a derivation environment variable and from there a
`.drv`. Nothing downstream can tell: the build gets a path that does not exist,
or worse, one that does and is not the one the expression named. There was no
error to grep for and no test that would have gone red.

The third one is an ordering difference. Elements were all forced and then all
coerced, so an element that could not coerce reported whatever a *later*
element threw. Under `tryEval` that is a different value, not a different
message.

Then the same question, asked of the change itself -- "what did I just do to
argument ordering?" -- turned up a fourth, wider than the first three
(ENG-12674). Every builtin in this crate forces all its strict arguments before
the body type-checks any of them, so:

    builtins.tryEval (builtins.concatStringsSep 1 (throw "x"))
    cpp:  error: expected a string but found an integer: 1   (evaluation dies)
    rust: { success = false; value = false; }

cppnix's type error is uncatchable and a `throw` is catchable, so a program
branching on `success` takes the other branch. Confirmed on four builtins.

### The rule

A reported bug names one symptom, and a symptom is a place, not a boundary. The
loud defect and the quiet one live in the same function because they have the
same cause: nobody had read that function against its specification. So when a
bug is traced to a function, the unit of work is the function against the
spec, not the symptom against the fix. Reading `prim_concatStringsSep` took ten
minutes and found three defects where the ticket described one.

The corollary is the one that produced ENG-12674: after the fix, ask what the
fix changed about everything the function touches that the ticket never
mentioned. Ordering, laziness, error class, context propagation. That question
found a divergence in four other builtins.

### And then the same thing happened to the fix

Two independent reviews of the change came back with one finding each, the
same one: the argument the fix rested on was true of cppnix and false of this
evaluator. The commit justified coercing element by element by saying cppnix's
call-depth guard is released per element, so one shared budget would overflow
on a long list -- and `Coerce`'s counter was indeed a counter over the whole
walk rather than a depth, which refused
`toString (genList (i: { outPath = "x"; }) 12000)` where cppnix answers 23999,
and, because it counted only attribute sets, accepted a 20,000-deep nested list
that cppnix refuses. Wrong in both directions, pre-existing, and invisible
until a change asserted the invariant it violated.

Fixing it deleted the fix's own reason for its shape. With the depth on the
work item, a single coercion over the whole list is correct too, so the
continuation written to avoid the shared budget became 32 lines of nothing, and
the second constructor beside it was byte-identical to one that already
existed. That is the part worth keeping: **a design comment is a claim about a
constraint, and removing the constraint does not remove the comment.** The
comment is what made this findable -- it stated the invariant plainly enough
that two reviewers could check it and find it false. A vaguer one would have
survived both reviews and the simplification would never have been noticed.

## "X is not in the tree" is a claim with a timestamp

A negative result about a moving ref is only true at a revision, and stating it
without one turns "I did not find it" into "it does not exist".

Two of us did it inside an hour on 2026-08-06, in opposite directions, and both
of us got away with it by luck rather than care.

Asked to review a purity-table row, I ran `git fetch` and then
`git grep WriteDrv PathReads origin/ix-patched`, found nothing, and replied that
neither symbol existed in the tree, so I could not review from anything but the
author's paraphrase. Both were there. Two merges had landed between my fetch and
their reading of my message, and my sentence carried no revision, so what was an
honest observation at `a2d749e4d` read as a statement about the present. Had I
written "not present at `a2d749e4d`" the author would have spotted the staleness
in one line instead of having to go and disprove it.

In the other direction, the same author told me a conflict-marker check was
clean, also with no rev. It happened to be true. Nothing in the report would
have distinguished that from a check run against a ref predating the bad commit.

The rule this belongs beside is already in this repo's habits for the positive
case: print the store path of any Nix-computed input next to the result that
depends on it, because a stale input produces a confident wrong measurement. The
negative case is the same rule with the sign flipped and it is *easier* to get
wrong, because a positive result carries its own evidence -- you have the thing,
you can print its hash -- while a negative result is an absence and has nothing
to attach a hash to except the ref you looked in. So the ref has to be attached
deliberately.

It is also the shape of "a zero deserves more suspicion than any other number"
(under "The rule this is all one instance of") one level up. There, a query that
matched nothing looks exactly like a query that found nothing. Here, a tree that
did not contain it yet looks exactly like a tree that never will.

Concretely, when reporting that something is absent:

- Name the rev: `git grep <pat> $(git rev-parse --short origin/ix-patched)`,
  and put that short sha in the sentence.
- Prefer an explicit rev over a remote-tracking branch name in the report. The
  name is a variable and the reader resolves it at their clock, not yours.
- If the absence is load-bearing -- a review declined, a feature declared
  missing, a ticket closed as not-applicable -- re-check at the moment of
  writing, because the gap between "I looked" and "I said" is exactly where the
  merge lands.
