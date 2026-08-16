# What a retained evaluator costs the ix CI gate, and why the pool was not built

Measured 2026-08-05 on dev-compute-1 (32 cores, 125 GiB) against ix
`fc9a2bba02` and this fork's `2.34.7+ix.g69e4d9e9db39`. Written because the
numbers point at an obvious next step that should not be taken, and the next
person to look at this will otherwise rediscover the same dead end.

## The gate already has most of the within-run reuse

The required-CI gate is one `nix-eval-jobs --flake
.#requiredCiJobScopes.x86_64-linux.full --workers N` pass over **680
attributes** (the gate script's own comment still says 185; it is stale).

`nix-eval-jobs` forks N workers and **each worker holds one `EvalState` alive
across every attribute it is handed**. So a run already shares evaluation with
itself. What a single `nix eval-persistent --retain` adds is *full* sharing and
the loss of *all* parallelism, and that is a trade, not a win.

One retained evaluator, serial, against a fresh 8-worker fan-out of the same
tree, all 679 attributes that produce a drvPath:

| pass | wall | vs fan-out |
| --- | --- | --- |
| retained, cold | 610s | 2.2x slower |
| retained, warm repeat | 250-286s | about par |
| `nix-eval-jobs`, 8 workers | 278s | — |

**Eight workers is not what CI runs, and against the number it does run the
comparison inverts.** `derive_eval_workers` computes `(cap_mb -
eval_non_worker_reserve_mb - eval_heaviest_attr_mb) / eval_worker_cost_mb`,
which on the medium runner's 64 GiB is **2**. Measured on dev-compute-3, same
box, same tree, back to back:

| pass | wall |
| --- | --- |
| `nix-eval-jobs`, **2 workers** | 863s |
| retained, cold | 597s |

So a *serial* retained evaluator answers the whole set in 69% of the time the
fan-out CI actually configures takes. Read that as an upper bound, not a
result: the oracle runs first in this harness and warms the store, IFD outputs
included, so the retained pass that follows is doing less work than the oracle
did. The ordering favours the evaluator and I did not control for it. The
honest statement is that the 8-worker comparison overstates the fan-out, the
2-worker one probably overstates the evaluator, and the truth is between.

The warm rows are steady across seven repeats with no downward trend, so a warm
retained pass costs what it costs. **Do not quote the fork's "0.4% of cold"
figure for this**: that measures re-asking *one* attribute. Asking all 679 again
is 41% of cold, not 0.4%, and the difference is the whole argument.

Two caveats on the table above, both of which cut in the retained evaluator's
favour and neither of which was measured:

- CI does not run 8 workers. `derive_eval_workers` computes `(cap_mb -
  eval_non_worker_reserve_mb - eval_heaviest_attr_mb) / eval_worker_cost_mb`,
  which on the medium runner's 64 GiB is **2**. Against a 2-worker fan-out the
  retained evaluator should look better than it does here.
- The dev box was doing IFD rebuilds after an auto-GC (ENG-12420), so absolute
  numbers are not comparable to CI's warm store. The ratio between two arms
  measured back to back on the same box is what to read.

## Agreement is not the problem

Eight consecutive full passes, every answer compared against a fresh
`nix-eval-jobs` answer for the same tree:

```
round  kind          wall_s    agreed diverged  unverif
1      untouched        610       679        0        0
2      untouched        250       679        0        0
...
8      untouched        286       679        0        0
```

**5,432 comparisons, zero divergences.** Every one of those rounds is an
*untouched* tree, which is the easier half: no edited round ever ran, so
read-set invalidation -- the part with the escapes named in
`read-set-recall.md` -- is unexercised by this measurement. Nothing here says
invalidation works.

## Memory is the reason the pool is a different project

Peak RSS, read from the kernel's own `VmHWM`:

| point | peak RSS |
| --- | --- |
| 73 answers into the first pass | 10.1 GiB |
| inside `host-generated-scripts` | 19.5 GiB |
| inside `index-eval` | 30.1 GiB |
| after pass 2 | 37.8 GiB |
| during pass 8 | 47.8 GiB |

Two shapes in that table. The peak is set by **individual heavy attributes**,
not by the count -- `host-generated-scripts` alone moves it 13 to 19.5 GiB and
the next 237 attributes add 0.4 GiB between them, so extrapolating the early
slope (which predicted ~50 GiB) is wrong by more than 2x. And the working set
**grows about 1.5 GiB per pass on a tree that is not changing**, because
retention means nothing is ever freed.

That second one is what makes a long-lived pool a memory-management project
before it is a speed one. It also has teeth: running an 8-worker fan-out beside
a 40+ GiB retained evaluator took the 125 GiB dev box down hard enough to need
a power cycle (ENG-12461). The gate's `eval_worker_cost_mb` of 16384 -- a
*measured* per-worker cost, not the 4096 the `--max-memory-size` flag requests
-- is the number to do that arithmetic with.

## The edited pass does not terminate

The finding that matters most, and it arrived last. Everything above is an
*untouched* tree. The first attempt at an **edited** one -- a one-octet
`publicIpv4` change to an inventory node, oracle recomputed, all 679 re-asked --
reached answer 664 of 679 and stopped, on `seam-secret-store-runtime-graph`, for
the fifty minutes I left it before killing it. In the untouched pass that same
attribute is a small fraction of a 597s total.

It is thrashing, not computing. RSS flat at 63.3 GiB throughout, 81% of one core
burned continuously, zero answers emitted, and:

```
# major faults, 30 seconds apart
before: 3720134
after:  3787100          # ~2,232 major faults per second

$ cat /proc/pressure/memory
full avg10=29.40 avg60=28.46 avg300=27.08

$ free -g | tail -1
Swap:  126  0  126       # not swapping -- these are file-backed pages
```

The Boehm mark phase walks a 63 GiB heap; the heap crowds out the page cache
that same walk needs; forward progress collapses. Note the footprint: an
untouched pass settles at 37.3 GiB and the edited one reaches 63.3 GiB,
consistent with retaining both pre-edit and post-edit values. The growth
documented above is the benign case, and an *edit* is what carries it into the
range where it stops working.

**So read-set invalidation remains unmeasured.** Not disproven and not divergent
-- 5,432 compared answers across nine untouched passes are all exact -- but
unreachable, because the pass that would test it does not finish. Treat
non-termination as the blocking item, ahead of the escapes named in
`read-set-recall.md`. ENG-12468.

The cheapest way to separate "invalidation is wrong" from "invalidation cannot
be reached" is a smaller attribute set, a few dozen including one heavy one so
the heap stays under ~20 GiB, and an edited pass over that.

## The other blocker

`nix eval-persistent` has **no per-attribute error isolation**. `nix-eval-jobs`
evaluates each attribute in a forked worker and reports a failure as one failed
entry while the rest of the set continues; the retained evaluator throws and the
session dies, taking the retained graph with it. At `fc9a2bba02` the attribute
`deploy-surface-ci-dispatcher` genuinely fails to evaluate, and feeding the raw
680 to a retained evaluator killed it at request 680 of 680. ENG-12424.

A CI evaluator cannot do without this, so it blocks any attempt to *substitute*
for the fan-out rather than shadow it.

## The verdict

The shape that could beat today's gate outright is a pool of K retained
evaluators, sharded by attribute, persisting across runs. **It was not built and
should not be.** Per this repo's standing priority the C++ evaluator is the
bridge and gets correctness fixes only; new effort goes to the Rust VM. The
measurements above are what make that a cheap call rather than a matter of
taste: the pool would have to solve unbounded retention growth, make an edited
pass terminate at all, and add per-attribute error isolation, before it
delivered its first second of speedup.

What did ship is a shadow: indexable-inc/ix#9903 runs the retained evaluator
beside the gate on the nightly schedule, compares every answer against the
fan-out's, and never lets it decide anything. That accrues cross-run soundness
evidence at one run a day, which is the cheapest way to keep the option open.

## Reproducing

Harnesses are on dev-compute-1 under `~/cipilot` (`rounds.sh` is the round
suite, `gate-probe.sh` the first baseline). Two dev-node traps that cost hours
and are not this fork's fault: the store sits near `min-free` so nix's auto-GC
fires mid-evaluation and invalidates timings (ENG-12420), and logind runs
`KillUserProcesses=true`, so `setsid nohup` jobs die on ssh logout and nix
reports it as `error: interrupted by the user` (ENG-12445). Hold the session
open or use `systemd-run --user`.
