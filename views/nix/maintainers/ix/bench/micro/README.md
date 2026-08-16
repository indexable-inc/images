# Pure-eval micro benchmarks: the M2.5 go/no-go record

Five expressions both backends evaluate identically (verified before
timing), timed with hyperfine (1 warmup, 5 runs) on hydra
(aarch64-darwin), build sha256 1ae9e492c968 (ix-patched @ 044b6187e,
rust-eval enabled). Startup floor measured with `--eval -E 1` and
subtracted: cpp 203ms, rust 200ms.

| bench   | cpp work | rust work | rust/cpp |
|---------|---------:|----------:|---------:|
| fib     |    448ms |      91ms |    0.20x |
| fold    |     87ms |      47ms |    0.54x |
| attrs   |     37ms |     186ms |    5.03x |
| strings |     29ms |     274ms |    9.51x |
| sort    |     11ms |     679ms |   64.11x |

Verdict: GO. The prior art's central risk (tvix measured 5.7-11.4x
slower than cppnix overall) does not reproduce here: the explicit-frame
VM beats cppnix 5x on call-heavy and 2x on fold work. The three slow
paths are named mechanisms, not the architecture:

- sort: the deliberate O(n^2) insertion-sort stopgap in primops_pure.rs
  (throwing comparators forced the simple shape); replace with a stable
  merge sort driven through the continuation machine.
- attrs: BTreeMap<Sym, Slot> plus per-name interner round trips versus
  cppnix's sorted arrays; the planned two-word value + sorted-vec
  representation work.
- strings: Rc<str> reallocation churn in toString/concat paths; a
  rope-or-builder path in ConcatStrings and coerce.

Regenerate: run the .nix files through both arms of one binary (the
capability probe rule applies; see lang-diff.sh), verify outputs agree
before timing anything, subtract the -E 1 floor per arm.

## The builtins-reference pair, and what it is for

`builtins-ref.nix` and `builtins-ref-hoisted.nix` are the same 400k-iteration
loop, differing only in whether `builtins.stringLength` is written inside the
loop body or lifted into a `let`. The gap between them is the cost of a
`builtins.<name>` reference and nothing else: they are supposed to have the same
time, and a version of the evaluator where they do not has put per-reference work
back on the hot path.

**Nothing runs these on a schedule, and no gate fails when the gap reopens.** They
are here to be run by hand, by whoever is about to claim something about
`builtins` references. What is guarded automatically is the mechanism rather than
the timing, in `nix-eval-rs`: `a_named_builtin_does_not_build_the_set` fails if
the compiler goes back to emitting `Op::BuiltinsSet` for a named reference, and
`the_builtins_set_is_built_once_per_vm` fails if the `Vm` stops sharing the set.
A change that keeps both of those green and is still slow would not be caught by
anything but running these files.

At `855e9b4d5` the gap was 28x (6.197s inline against 0.219s hoisted, minimum of
three, evaluator time only, hydra), because `Op::BuiltinsSet` rebuilt the
~160-entry attrset on every execution and the compiler emitted it for every
reference. ENG-12539 closed it in two commits: `5289f6b7d` gave the `Vm` one set
instead of one per reference, which took the bulk, and the compile-time fold took
the rest. At `88535e14b` the pair measures 0.200s and 0.191s on the same machine.

Quote a number from here only with the revision beside it, as above. These get
stale every time the evaluator moves, and the reason to keep the pair rather than
either figure is that the *gap* is the property; the absolute times are whatever
the machine was doing that day.

`hashstring.nix` is the rung E expression from
[rust-default-ladder.md](../../rust-default-ladder.md), kept here so the
measurement has an input rather than a quotation. `hashstring-small.nix` is the
same shape at 1/100th the iteration count, which is the one to reach for: the
full expression takes tens of seconds per arm.

The inputs are deliberately scalar. `builtins-ref*.nix` and `hashstring*.nix`
add integers, so `Op::Add` stays on the arithmetic path; ENG-12593 made `+`
coerce a set operand through `__toString`/`outPath`, which is a call, so an
operand that is a derivation or any other set turns each `+` into arithmetic
plus a task round trip. Putting one into these files would keep the bench
reporting under the same name while measuring a different shape, which is the
one way these numbers can go wrong quietly.

These four are timed with the crate's `compile-share` example rather than with
hyperfine, because it reports evaluator time separately from compile time and
runs in-process, so there is no startup floor to subtract on the rust side. The
cpp arm still has one, so subtract `nix eval -E 1` from it before comparing.
