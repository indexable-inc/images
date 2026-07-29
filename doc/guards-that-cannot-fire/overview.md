# Guards that cannot fire

A guard that cannot fire is worse than no guard, because the documentation
next to it tells the next reader that something is being watched. Ten were
found on 2026-07-29, all by hand, all by somebody who happened to look. This
is an attempt to stop finding them that way: what the class actually is, how
much of it can be enumerated mechanically, and which parts of it no gate can
catch.

The short version, so a skimmer can stop here:

- The ten instances are **four distinct shapes**, not one, and they need
  different gates. Lumping them produces one gate that catches none.
- **Two shapes can be enumerated from history today.** 44 of 122 CI checks and
  9 of 15 guard-named fleet units have never once fired.
- **One shape cannot be enumerated at all**, by any tool we have or could
  reasonably build. It is also the most common of the four.
- The only gate that covers all four is expensive: **a guard ships with a
  demonstration of its own failure.** For one shape that demonstration is the
  only thing that works.

## The four shapes

Named from the ten real instances rather than from a definition.

### 1. Never runs

The guard is never evaluated. Its condition may be perfect; nothing asks it.

- An assertion placed inside a list element that is never forced.
- A pre-commit hook documented as running the linter, which has never run it
  on any platform.

### 2. Cannot match

The guard runs, and its condition is unsatisfiable — or satisfiable only
outside anything reality produces.

- A predicate whose device exclusion made it match zero rows in thirty days,
  including the one incident it was written for.
- A check whose expected count silently changed from 2 to 3, because `f "x"`
  inside a Nix list is two elements rather than an application.
- A threshold that decays to zero when absent, where zero means detection off.

The last two are the same trick from opposite ends: one has the wrong constant
on the right-hand side, the other manufactures a left-hand side that can never
exceed it. **A default value that means "no problem" is a guard turning itself
off**, and it reads identically to a healthy system.

### 3. Cannot discriminate

The guard runs, matches correctly, and observes something that is the same in
both the world it is checking for and the world it is not. This is the largest
group and the one no tool can find.

- An address-resolution probe on a provider that answers for every address,
  including a reserved range that is not ours.
- A proxy health check that reports the process alive while its backend is
  unreachable, because the listener binds at startup and the backend is
  dialled per connection. Twelve hours of green.
- A gate keyed by port with no host dimension, so it cannot represent the same
  port on a second host and will not object to a new exposure.
- A fixture reaching for the repository's root commit as "a base with no
  data" — true locally, false in CI, so it silently exercised the ordinary
  path instead.

What unites them is that **the observation is a proxy for the property, and
nobody checked that the proxy moves when the property does.** A listener
binding is a proxy for a working proxy. An ARP answer is a proxy for delivery.
A port is a proxy for an exposure. Each is correct in the healthy case and
uninformative in the failing one, which is exactly backwards.

### 4. Cannot be heard

The guard runs, discriminates, fires — and the signal is lost.

- A build-time assertion whose one-line diagnostic never reached the log
  inside a parallel build, so it failed correctly and said nothing.

Also, arguably, everything the fleet notification work was about: a correct
alert delivered over a transport the client receives and never surfaces is the
same defect at a different layer.

## What can be counted, and what has ever fired

Populations counted from the trees on 2026-07-29 (`index` at 797 `.nix` files,
`ix` at 570 excluding the vendored `index` submodule).

| population | index | ix | total | ever-fired knowable? |
|---|---|---|---|---|
| `assert` expressions | 108 | 273 | 381 | **no** |
| `lib.assertMsg` | 115 | 282 | 397 | **no** |
| `throw` / `abort` | 164 | 305 | 469 | **no** |
| `assertions = [...]` entries | 24 | 54 | 78 | **no** |
| check derivations / `passthru.tests` | 159 | 192 | 351 | partially |
| systemd health hooks (`ExecStartPost`, probes) | 4 | 33 | 37 | via unit state |

Two populations *can* be answered from history, and both answers are alarming.

**CI checks — 44 of 122 have never failed.**

```
checks: 122   ever_failed: 78   never_fired: 44   (36.1%)
```

Several have five figures of runs: `classify` 12,996; `scope` 11,517;
`cleanup-branch` 11,089; `gate` 9,995. Some of those are actions rather than
checks and cannot fail by design. Others are not, and nobody has separated
them.

**Fleet units whose name says they are a guard — 9 of 15 have never failed.**

```
guards: 15   ever_failed: 6   never_fired: 9   (60%)
```

Topped by `ix-mdraid-membership-validate.service` at **213,327 observations
across 5 hosts**, and `ix-mdraid-logical-block-size-validate.service` at
88,116.

For context, across *all* 20,793 units ever sampled, only 96 have ever been in
`active_state='failed'` — 0.5%. That number is not a finding on its own; most
units are not guards.

### Reproducing it

Read-only, against the ClickHouse leader (`hil-compute-2`). A guard in these
outputs is a candidate, not a defect; see the next section.

```sql
-- CI checks: population, and how many have ever failed
SELECT count(DISTINCT job_name) AS checks,
       countDistinctIf(job_name, conclusion = 'failure') AS ever_failed,
       count(DISTINCT job_name) - countDistinctIf(job_name, conclusion = 'failure') AS never_fired
FROM kpi.ci_jobs;

-- ...the ones that never have, with enough runs to mean something
SELECT job_name, count() AS runs, min(created_at) AS since
FROM kpi.ci_jobs
GROUP BY job_name
HAVING countIf(conclusion = 'failure') = 0 AND count() >= 20
ORDER BY runs DESC;

-- fleet units whose name says they are a guard
SELECT count(DISTINCT unit) AS guards,
       countDistinctIf(unit, active_state = 'failed') AS ever_failed
FROM metrics.systemd_unit_health
WHERE match(unit, '(check|probe|verify|validate|guard|assert|drill|smoke)');

-- ...the ones that have never failed
SELECT unit, count() AS samples, countDistinct(node) AS nodes
FROM metrics.systemd_unit_health
WHERE match(unit, '(check|probe|verify|validate|guard|assert|drill|smoke)')
GROUP BY unit
HAVING countIf(active_state = 'failed') = 0
ORDER BY samples DESC;
```

These are deliberately four queries in a document rather than a script: the
repo fences new committed shell (`shell-allowlist.txt`, #3823) and its
allowlist only shrinks. Which is worth recording here, because **the fence is
a guard that fired correctly on this very change** — it discriminated, it
objected, and its message named both the rule and the alternative. Guards do
work when they are built to distinguish something real.

## A never-fired guard is a candidate, not a defect

This has to be said loudly or the audit will do more harm than the class does.

`ix-mdraid-membership-validate` has 213,327 observations and has never fired.
Reading it, it is **correct**: it walks each declared array, resolves the
member device, compares against `/sys/class/block/<md>/slaves`, and exits 1 on
mismatch. It has never fired because the disks have never been mis-assigned.
That is a guard working exactly as intended.

So the mechanical signal has a high false-positive rate by construction, and
that is not a flaw in the query — it is the reason the gate cannot simply be
"never fired means broken". What the enumeration buys is **42 things to read instead of 1,200** (33 CI
checks with meaningful run counts, plus 9 fleet guard units).

## The gate, per shape

There is no single gate. Proposing one uniform mechanism would be the same
error as treating the ten instances as one shape.

| shape | gate | mechanical? |
|---|---|---|
| 1. Never runs | evaluation coverage: did this assertion ever get forced | **yes**, per-guard |
| 2. Cannot match | run the predicate over full retention; zero hits is a candidate | **yes**, fleet-wide |
| 3. Cannot discriminate | a demonstration of the guard failing | **no** |
| 4. Cannot be heard | assert the diagnostic appears in the captured output | **yes**, per-guard |

**Shapes 1, 2 and 4 admit cheap mechanical gates** and should get them. Shape 2
already has one, in the script beside this document; it wants scheduling and a
triage owner, not more engineering.

**Shape 3 admits no mechanical gate, and it is the largest group.** To know
that an ARP answer distinguishes delivered from undelivered, you have to know
what the provider does with a reserved address — which is a fact about the
world, not about the code. No static analysis, no coverage tool and no history
query can supply it. The only thing that establishes it is someone
constructing the failing world and watching the guard object.

So for shape 3 the gate is a convention, and conventions need teeth:

> **A guard is not done until you have watched it fail.** Break the thing it
> protects, confirm it fires with the message you intended, restore. Put that
> demonstration in the tree next to the guard, so the next person can re-run it
> rather than trust it.

This already exists as a line in `CLAUDE.md`. It is not followed, and this
document is the evidence. The realistic improvement is not a stronger rule but
a cheaper one to obey: for the fleet-alert case the demonstration was four
lines of test, and it found the defect immediately.

## The rule this cost the most to learn

> **A suppression and a predicate must be measured separately. Verifying that
> the false alarm stopped tells you nothing about whether the alert can still
> fire.**

The tenth instance was written by someone actively trying to avoid this
category, in a module whose own documentation warns about it. A device
exclusion was added to silence a false alarm; the false alarm stopped; nobody
re-measured whether anything could still match. Zero hits in thirty days,
while the documented rate beside it claimed one a month. Both facts were true
at once and only one was checked.

The generalisation beyond suppressions: **when you narrow a guard, the
measurement you owe is on what it still catches, not on what it stopped
catching.**

## The corollary about tests

> **A layer that every test mocks is a layer with no tests.**

Every test of the fleet predicates stubbed the query function, which made the
SQL the one layer nothing looked at, which is where the dead exclusion lived.
Two tests now assert on the query text itself. Wherever a boundary is
universally mocked — SQL strings, generated shell, rendered unit files,
templated config — that boundary needs at least one test that reads what is
actually emitted.

## Worked demonstration

The gate for shape 2 caught a real instance during this work, which is why the
shape is characterised the way it is. `kernel_storage` in
`packages/mcp-ex/lib/ix_mcp/fleet/alerts.ex` matched zero rows in thirty days;
running the shipped SQL text over widened history returned 0, and removing one
clause returned the 3 real lines. Fixed in #4399, with the two SQL-text
assertions added in #4401.

Per the brief, no further instances were fixed here. They are listed below with
locations so they can be scheduled.

## What could not be enumerated, stated plainly

- **Every Nix-evaluated guard.** 856 `assert`/`assertMsg`/`assertions` forms
  across the two repos, and there is **no record anywhere of one having
  fired**. Build logs are not shipped to ClickHouse, and a failed evaluation
  leaves no durable trace beyond the developer's terminal. This is the single
  largest population and the one with the least visibility; closing it needs
  build-log retention, which does not exist today.
- **Lint and pre-commit stages.** `nix run .#lint` runs 15 tasks; no history of
  which have ever failed is retained.
- **Shape 3 across the board.** By definition. The four known instances were
  each found by a person reasoning about what the observation could not
  distinguish, and nothing here changes that.
- **Antithesis properties** were not enumerated; they have their own reporting
  surface and were out of scope for one session.

## Candidates, with locations

Not fixed. Filed for scheduling as **ENG-11262** (triage the 42) and
**ENG-11263** (the 856 Nix guards with no fired-record at all).

**CI checks that have never failed** — 44 in total, of which **33 have 20 or
more runs**. Top by volume:
`classify` (12,996), `scope` (11,517), `cleanup-branch` (11,089),
`respect-manual-merge-label` (10,069), `gate` (9,995), `disable-when-unsafe`
(9,641), `metadata-sync` (2,591), `attest` / `findings` / `sarif` (2,032 each),
`ungated-merge` (1,453), `main-red` (1,453), `ci-budget` (1,243),
`fuzz` (570), `unit guards` (211). Triage needed to split "is an action, cannot
fail" from "is a check, does not work".

**Fleet guard units, never failed:** `ix-mdraid-membership-validate` (213,327
samples — read, correct, true negative), `ix-mdraid-logical-block-size-validate`
(88,116), `ix-guest-egress-verify` (4,473),
`ix-orchestrator-memory-guard-v2` (365), `ix-deploy-guard` (119),
`ix-vm-public-ingress-verify` (55).

**Known shape-3 instances already identified elsewhere:** the ARP delivery
probe in `ix:nix/modules/providers/vrack-public-block.nix`, and the
port-keyed exposure gate with no host dimension.
