# The fleet-shaped shadow run: 2853 real attributes, zero value divergences

E2E-4. `maintainers/ix/shadow-mode.md` reports 262 language-corpus cases and
says plainly what it is not: "One workload, and it is the language corpus ...
A nixpkgs or ix-fleet evaluation would weight the refusal histogram completely
differently and is the run that should decide the default flip. This is not
that run." This is an attempt at that run.

This has been run twice, on two binaries, and **every table says which**:

    run 1   fork rev 770f0f0fa
            nix-instantiate sha256 114c6f000082c9ca9190ef1057637dcde14749e6a0980c82cd11f03da904ee1e
    run 2   fork rev 77b95143b, after ENG-12852 and ENG-12855 landed
            nix-instantiate sha256 720fafc8f88bc75bce1c31ece0a30a343157ae537dc1a7999bfe31d885d38bac

    build   -Dnix:rust-eval=enabled, debugoptimized, meson/ninja
    host    dev-compute-6, Linux x86_64, 32 cores, both runs
    claim   ix-dev-claim, actor shadow-fleet, both runs

Run 2 is the current picture and is summarised first. Run 1 is kept below
because two of its rows are the reason run 2 exists.

The binary hash is quoted beside every number because a checkout build's
`--version` carries no revision, and because this report contains a
before/after where the two arms are different binaries.

## The short version: one blocker, and it is not any of the 70 tier-2 rows

Over the workloads the Rust arm could reach it evaluated 2793 attributes and
**disagreed about a value zero times**. Reading the divergence total alone
would be a mistake, so the counts are split by what they mean for the flip:

| | count | blocks the flip? |
| --- | --- | --- |
| `value-mismatch` | **0** | would, if any |
| Tier 1 byte differences (outPath, drvPath) | **0** | would, if any |
| `rust-failed-cpp-succeeded` | 9 | **YES, ENG-12854** |
| `error-text-mismatch` | 70 | **no**, tier 2 by CLAUDE.md |

Per the parity bar (CLAUDE.md, #137), tier 2 covers error wording, render
format and trace shape, and a byte difference there is not a failure. The 70
`error-text-mismatch` rows are all one cause and none of them is a reason to
delay anything. The number that gates the flip is **9**.

Coverage, which is half of every claim above:

At **run 2** (`720fafc8`), the pinned tree evaluates end to end:

| workload | attempted | rust evaluated | value-mismatch | flip-blocking |
| --- | --- | --- | --- | --- |
| nixpkgs, pinned 25.11 | 2638 | **2638 (100%)** | **0** | 9 |
| nixpkgs, unstable | 2638 | 2638 (100%) | **0** | 9 |
| ix fleet inventory (run 1) | 215 | 215 (100%) | **0** | 0 |

**Zero refusals in either nixpkgs arm.** The refusal histogram, which had 60
rows on unstable and 2638 on the pinned tree at run 1, is now empty. What
changed between the runs, same list, same host:

| | run 1 (`114c6f00`) | run 2 (`720fafc8`) |
| --- | --- | --- |
| pinned: rust evaluated | 0 (0.0%) | **2638 (100%)** |
| pinned: `path-interpolation` refusals | 2638 | **0** |
| unstable: `unimplemented-builtin` refusals | 52 | **0** |
| unstable: `path-interpolation` refusals | 8 | **0** |
| either arm: `value-mismatch` | 0 | **0** |
| either arm: flip-blocking | 9 | 9 |

ENG-12852 removed the pinned wall; `builtins.filterSource` (e1828ec40)
removed the 52. The `path-interpolation` token itself has since been deleted
from `RefusalToken`, so the rows naming it in this document are historical:
they describe binaries that predate the implementation, and no run after this
one can produce that name at all. The blocking count did not move, because ENG-12854 was still
in flight when run 2 was measured.

Four tickets came out of this run. One blocks the flip (ENG-12854), one walls
the pinned tree (ENG-12852), one is a gate that has been measuring the wrong
tree (ENG-12855), and one is a defect in the measurement machinery itself that
silently undercounted this very report (ENG-12874).

## No zero here is trusted until the comparator was watched firing

Run once per environment before anything else, because a comparator that never
fires produces the same report as a healthy backend.

`builtins.stringLength` was made to return `len + 1`
(`rust/nix-eval-rs/src/primops_pure.rs:1446`), rebuilt, and the binary hash
moved to `9a2a4d283e72...`, which is itself part of the check: without it the
next paragraph could be describing the old binary.

Two things were then confirmed, not one:

1. **The feature fires.** `nix-instantiate --eval --strict -E
   'builtins.stringLength "abcde"'` printed `5` to the user and to stderr:

       <4>rust-eval shadow divergence kind=value-mismatch id=63ae64279f95 origin=<expr> cpp=5 rust=6

2. **This report's own harness fires.** The 30-attribute prefix of the sweep
   list, run through `shadow-nixpkgs-sweep.sh` against the seeded binary,
   reported `attempts=30 refused=0 evaluated=30 divergences=30 distinct=30`.
   Checking only (1) would have proved the evaluator's comparator works while
   saying nothing about the aggregation this document quotes, which is the
   layer that reported every divergence twice when shadow-mode.md was written.

Reverted, rebuilt, and the binary hash returned to
`114c6f000082c9ca9190ef1057637dcde14749e6a0980c82cd11f03da904ee1e`, byte for
byte. Every number in this document is from that binary.

### The divergence id is portable, verified rather than assumed

`63ae64279f95` above is the same id shadow-mode.md records from a macOS
worktree. That is the cross-machine half.

The cross-path half was checked separately, because the seeded exhibit has
`origin=<expr>` and carries no file at all. Two runs of the same six
attributes, each from its own `mktemp -d`, so the wrapper's absolute path
differed:

    run 1: /tmp/tmp.7io1a4NXA3   run 2: /tmp/tmp.pz1e1rbYjx
    ids: 119d0d3f7ef6 985153abc0ef bb52ddb59164 cad5f318b742 dd85dfe07472 fc8dd2e5b5d3
    identical across both runs

So the #133 fix holds for the file case as well as the expression case. No
regression to one id per host.

### The wide list is a superset, so the wider run has to reproduce the narrower

The sweep was first run at 692 attributes and then widened to 2638.
`shadow-nixpkgs-attrs-wide.txt` is a strict superset of
`shadow-nixpkgs-attrs.txt`, checked mechanically rather than by construction,
and the point is the consistency check it buys: the wider run must find every
divergence the narrower one found.

    narrow (692) distinct divergences: 23
    wide  (2638) distinct divergences: 79
    narrow ids NOT reproduced by the wide run: 0

A wider run that quietly lost one would be a harness bug wearing a better
denominator.

## Workload 1a: 2638 nixpkgs attributes, unstable, at full depth

    maintainers/ix/shadow-nixpkgs-sweep.sh build-rust/src/nix \
      maintainers/ix/shadow-nixpkgs-attrs-wide.txt \
      --nixpkgs /nix/store/llgwlxshmy0ifvxh7f8wq53vk5x7vd13-source --jobs 12

851 packages stride-sampled from the 24,915 top-level derivation attributes,
three attributes each (`.name`, `.outPath`, `.meta.description`), plus 25
`.drvPath`, 61 lib and stdenv attributes, and 6 attributes that do not exist.

**Coverage, both ways.** Two columns because the run hit ENG-12874, described
below, which destroyed 7 processes' stats files. The recovered column is what
the run actually did; the counted column is what the machine-readable census
reported.

    attributes attempted            2638
    stats files read                2631      (7 destroyed by ENG-12874)
    shadow attempts                 2631      true 2638
    of those, the rust arm REFUSED    60
    of those, the rust arm CRASHED     0
    so the rust arm EVALUATED       2571      true 2578   (97.7% of attempts)
    unaccounted                        0
    rust arm time                3863.85s     COLD every call: ENG-12830
    wall clock                       397s     at -P 12

**Verdicts.**

    agreed                          2472
    agreed-failure                    27
    agreed-failure-text-differs       63
    crashed                            0
    mismatched                         9
    refused                           60

**Divergences, counted and corrected.**

    value-mismatch                     0
    cpp-failed-rust-succeeded          0
    rust-crashed                       0
    error-class-lost                   0
    error-class-mismatch               0
    error-text-mismatch               63      true 70
    rust-failed-cpp-succeeded          9

    distinct divergences              72      true 79

**Refusals, which are not divergences.** Two causes, both named, and each one
is a single construct rather than a spread:

    unimplemented-builtin             52      every one of them builtins.filterSource
    path-interpolation                 8      ENG-12852, present in unstable too

`unrecorded` does not appear, so every refusal that crossed the boundary
carried a name. Every `skipped` row is zero (budget, reentrant,
unservable-shape, cpp-failed-before-eval), so nothing was quietly excluded
from the denominator.

### The 9 that block the flip: ENG-12854

Every one is `.outPath`, and every one carries the identical Rust message
`eval: expected a string but found a path`:

    autoPatchcilHook  dotnet-ef  dotnet-runtime  everest
    godot3-mono-export-templates  n-m3u8dl-re  pinta  renode-bin  torrentstream

Reduced to two builtins:

| expression | cpp | rust |
| --- | --- | --- |
| `builtins.stringLength ./f.sh` | `48` | `error: expected a string but found a path` |
| `builtins.substring 0 4 ./f.sh` | `"/nix"` | `error: expected a string but found a path` |

48 is the length of the store path the file is copied to, so cppnix coerces
with `copyToStore` and the Rust arm does not. It reaches packages through
`lib.strings.hasSuffix`, which computes `stringLength content`
(`lib/strings.nix:852-864`) and is handed a path by `makeSetupHook`:

    (import <nixpkgs> {}).lib.strings.hasSuffix ".sh" ./f.sh
    cpp:  true
    rust: error: expected a string but found a path

Three sibling builtins were checked and are correct, both arms rejecting a
path: `match`, `replaceStrings`, `hashString`. Three more already coerce:
`concatStringsSep`, `baseNameOf`, `toJSON`. So the gap is exactly two
builtins, and the line separating them is whether the cppnix primop uses
`coerceToString(..., copyToStore)` or `forceStringNoCtx`. Fourth sighting of
one shape after ENG-12628, ENG-12669 and ENG-12670, which is the ticket's
argument for deriving the list from `src/libexpr/primops.cc` and asserting it.

**Nothing was silently wrong.** Across roughly 850 `.outPath` and 25
`.drvPath` attributes, tier 1 under the parity bar, the Rust arm never
computed a *different* store path. It either produced cppnix's bytes or
failed loudly.

### The 70 that do not block anything: ENG-12714

All 70 are a nixpkgs `Refusing to evaluate package` throw for an unfree,
broken or wrong-platform package. Both arms fail, with the same package name
in the message. cppnix names the file and line; the Rust arm carries no source
positions and says `«unknown-file»`:

    cpp  = Refusing to evaluate package 'qq-3.2.29-2026-05-28' in
           /nix/store/llgwl...-source/pkgs/by-name/qq/qq/package.nix:46 because it has an unfree license
    rust = Refusing to evaluate package 'qq-3.2.29-2026-05-28' in
           «unknown-file» because it has an unfree license

Tier 2. It is 70 rows rather than one only because the id includes the
attribute path.

## The run undercounted itself, and one line caught it: ENG-12874

Worth its own section because it is the failure shadow-mode.md singles out as
the one worth guarding against most: "a failure of the comparison machinery
itself, which is otherwise the one bug that would silence exactly the runs
that found something."

`shadowTruncate` (`src/nix/rust-eval-session.cc:1744`) cuts a divergence's
text at 200 **bytes**:

```cpp
return text.substr(0, limit) + "…(" + std::to_string(text.size()) + " bytes)";
```

nixpkgs' unfree message contains U+2018/U+2019 curly quotes in `because it has
an unfree license (‘unfree’)`. When byte 200 lands inside one of them the
result is invalid UTF-8, the `NIX_SHOW_STATS` JSON writer fails, and **the
whole process's stats file is written as zero bytes**: not one row, but that
process's attempts, verdicts and refusal tokens as well.

Whether the cut splits a character depends on the package name's length, which
is why 7 of roughly 90 otherwise identical unfree throws are affected. The raw
stderr shows the damage, a dangling `\xe2` immediately before the appended
ellipsis:

    INVALID UTF-8 at byte 770 :
    b'p-software/package.nix:16 because it has an unfree license (\xe2\xe2\x80\xa6(990'

Deterministic, not a race: three consecutive runs of each reproduce it. And
specific to shadow, which is what makes it the comparison machinery's bug
rather than the stats writer's. Same attribute, same binary, only the setting
differing:

| eval-backend | stats file size |
| --- | --- |
| `cpp` | 2874 bytes |
| `rust` | 2798 bytes |
| `shadow` | **0 bytes** |

The seven, all `.outPath`: `aja-desktop-software`, `httpie-desktop`,
`ivsc-firmware`, `nomad`, `pdfstudio2023`, `sonarqube-cli`,
`soundfont-arachno`. The `<4>` divergence line still reaches stderr, so all
seven were recovered by decoding stderr with replacement, and all seven are
`error-text-mismatch`. That is the source of every "true" column above.

**What caught it.** `shadow-nixpkgs-sweep.sh` prints `stats files read` beside
`attributes attempted` and fails the run when the first is smaller, because
"the file is missing" and "the file says zero" are different facts and only
one of them is a clean run. It was the only signal; every other number in the
report looked healthy, and the run exited 1 with `only 2631 of 2638 attributes
were shadowed at all`. The narrower 692-attribute run never tripped it, which
is the argument for the wider denominator.

## Workload 1b: the same 2638 attributes against the nixpkgs this fork pins

    --nixpkgs /nix/store/p5cm66j33sbpn8ni9f2hlr279sfhvgwq-source   # 25.11.6495, flake.lock

    attributes attempted            2638
    stats files read                2638
    shadow attempts                 2638
    of those, the rust arm REFUSED  2638
    so the rust arm EVALUATED          0   (0.0% of attempts)
    unaccounted                        0
    rust arm time                2563.80s   COLD every call
    wall clock                       290s   at -P 12

    path-interpolation              2638
    divergences                        0

**One construct causes all 2638.** `nix-eval-rs` refuses an interpolated path
literal at compile time (`compile_path`,
`rust/nix-eval-rs/src/compile.rs:264`). nixpkgs 25.11 has one on the load path,
at `pkgs/development/interpreters/python/cpython/default.nix:404`:

    ./${lib.versions.majorMinor version}/gh-142218.patch

Located rather than guessed: strace shows `import <nixpkgs> {}` opens 103
`.nix` files, and compiling each individually through the Rust arm refuses on
exactly one. Minimal repro, `let v = "3.13"; in ./${v}/gh.patch`; cpp resolves
it and the Rust arm refuses. ENG-12852. Different from ENG-12447, which was
string interpolation *of* a path (`"${./foo}"`); this is a path literal that
*contains* an interpolation.

**The harness refuses to let this read as a clean sheet.** Probe 3 evaluates
the package set root through both arms before scoring anything and prints the
refusal when the root does not serve; the run was invoked with
`--expect-refusal-token path-interpolation`, so a run that came back empty for
some other reason would fail rather than pass quietly.

ENG-12852 landed as `77b95143b` and this arm went from 0 evaluated to 2638,
all of it, in one step. The prediction written here was "roughly 2570"; the
run is reported above under run 2, as a first measurement of the pinned tree
rather than a before/after against this row.

### A gate has been measuring a tree nobody ships: ENG-12855

Found while isolating the above, and not a code regression, which took one
experiment to establish. `nixpkgs-frontier.sh` defaults `NIXPKGS` to the flake
registry, which floats. The same binary, two trees:

| nixpkgs | rows | agree | refused | exit |
| --- | --- | --- | --- | --- |
| registry `llgwlxs...` (26.11pre-git) | 12 | 12 | 0 | 0 |
| flake.lock pin `p5cm66j...` (25.11.6495) | 12 | 6 | 6 | 1 |

The pinned run prints `FAILED: 6 rows agree, the checked-in floor is 12. The
frontier went backwards.` It did not go backwards. The gate is comparing
against a different tree from the one its floor was recorded against, and it
has never measured the nixpkgs this fork pins, which is how a construct that
refuses the entire pinned package set stayed invisible. Note for whoever pins
it: the floor must be remeasured in the same commit, or the gate is red on day
one.

## Workload 2: the ix fleet inventory, 215 attributes, all agreed

The ix flake's own eval surface is still walled, with both walls named:

    builtins.getFlake        refused, token unimplemented-builtin
    nix eval <flake>#attr    refused, token command-installable

So this arm reaches the fleet data the way a non-flake caller does: `ix`'s
`nix/inventory` imported directly at a pinned snapshot of ix rev
`56f4197d3297d8582e8e7ba8afb668a6bff889a6`, giving 12 real fleet nodes across
3 regions, 17 fields each, plus the 8 top-level inventory sets.

    attributes attempted            215
    shadow attempts                 215
    of those, the rust arm REFUSED    0
    so the rust arm EVALUATED       215   (100.0%)
    unaccounted                       0
    rust arm time                240.57s   COLD every call
    wall clock                       25s   at -P 12

    agreed                          215
    all divergence kinds              0
    all refusal tokens             none

Real fleet inventory data, hostnames, regions, roles, network and deploy
configuration, evaluates identically on both backends with nothing refused.

## Run 2: the pinned tree, first measurement (rev 77b95143b, sha 720fafc8)

ENG-12852 and ENG-12855 landed as `77b95143b`. This is the run the flip should
cite for the pinned tree. It is a **first measurement of that tree**, not a
before/after against run 1: run 1 evaluated none of it, so there is no
like-for-like predecessor to compare against, only a wall that is now gone.

Three things were verified before any number was believed, none taken on trust
from the fix's own PR:

- The minimal repro from ENG-12852 (`let v = "3.13"; in ./${v}/gh.patch`) now
  answers identically on both arms.
- `nixpkgs-frontier.sh` against the pinned tree reads `rows=12 agree=12
  refused=0`, exit 0.
- **The seeded-divergence check was redone on the new binary**, because a new
  binary is a new environment and every zero below rests on it. Seeded
  `stringLength + 1`, hash moved to `723372f7e11c...`, both layers fired again
  (the exhibit gave `value-mismatch id=63ae64279f95`; the harness on 30
  attributes gave `evaluated=30 divergences=30`), reverted, rebuilt, hash back
  to `720fafc8...` byte for byte.

That id, `63ae64279f95`, is now stable across macOS, two Linux binaries and
three revisions.

### Pinned nixpkgs 25.11, 2638 attributes

    attributes attempted            2638
    stats files read                2638
    shadow attempts                 2638
    of those, the rust arm REFUSED     0
    of those, the rust arm CRASHED     0
    so the rust arm EVALUATED       2638   (100.0% of attempts)
    unaccounted                        0
    rust arm time               18819.62s   COLD every call
    wall clock                      1681s   at -P 12

    agreed                          2331
    agreed-failure                   237
    agreed-failure-text-differs       61
    mismatched                         9
    refused                            0

    value-mismatch                     0
    rust-failed-cpp-succeeded          9      ENG-12854
    error-text-mismatch               61      ENG-12714, tier 2
    every other divergence kind        0

    refusal tokens                  none
    distinct divergences              70

### Unstable, same list, same binary, as a control

    attributes attempted            2638
    stats files read                2631      7 destroyed by ENG-12874
    shadow attempts                 2631      true 2638
    of those, the rust arm REFUSED     0
    so the rust arm EVALUATED       2631      true 2638   (100.0%)
    unaccounted                        0
    rust arm time                3890.94s   COLD every call
    wall clock                       400s   at -P 12

    value-mismatch                     0
    rust-failed-cpp-succeeded          9      the same 9 packages
    error-text-mismatch               64      true 71
    refusal tokens                  none
    distinct divergences              73      true 80

The control is what makes the pinned numbers readable. Both arms now evaluate
100%, both find the same 9 blocking failures, and neither disagrees about a
value.

**ENG-12874 fired again, identically.** The same 7 attributes lost their whole
stats file, with the same 7 divergence ids as run 1 (`d7282c5e3228`,
`3a49d6e91646`, `25bff2787ec4`, `34c6adde5b21`, `9fd877aca8ad`,
`020448248c64`, `bdb418bc8057`), all `error-text-mismatch`. It is deterministic
and still unfixed; it did not fire on the pinned tree only because the package
name lengths there put the 200-byte cut somewhere harmless. Latent, not gone.

## The pinned run was 4x slower than the control, and it is a bug: ENG-12913

Worth its own section because the first explanation was wrong and the data
said so.

Pinned took 1681s of wall clock against unstable's 400s on the same list and
binary. The initial hypothesis was `builtins.filterSource`, newly implemented
in the same window, on the theory that a builtin which used to refuse
instantly now does real directory walks. **That was checked and is wrong**:
zero of the slow attributes were among the 52 that previously refused with
`filterSource`.

What the data actually says:

    total rust arm:        18820s over 2638 attrs
    attribute-not-found:     217 attrs, 14864s = 79% of the total
    everything else:        2421 attrs,  3956s, mean 1.63s

217 of the attributes are packages that exist in nixpkgs-unstable, where the
list was generated, and not in 25.11. Both arms agree they are missing
(`agreed-failure`, identical message). The Rust arm just takes about a minute
to say so. On an idle box:

| command | cpp | rust |
| --- | --- | --- |
| `-A absolute.name` | 192 ms | **59,013 ms** |
| `-A zzzznotarealname.name` | 222 ms | **59,135 ms** |

The two rust rows being equal is the diagnostic: if the cost were in scoring
near-matches, a nonsense name would be cheap. It is the *enumeration* of
candidate names for the "did you mean" suggestion, one index at a time through
the handle API, 25,442 times. A not-found against a small attrset is 1.1s, so
it scales with the set and not the query. ENG-12913.

**Excluding it, the two backends agree on cost**: 1.63s per attribute on
pinned against 1.48s on unstable. The pinned run is not slower because of the
tree; it is slower because the list points at 217 missing packages and each
one hits this.

## Overhead, and the cache state it is measured in

Every number here is **cold**. `eval-cache-dir` writes objects but never
serves them on the handle path (ENG-12830), and shadow evaluates through
`rustEvalSelect`, which is that path, so the Rust arm recomputes from scratch
on every invocation. This is an upper bound that should fall when ENG-12830
lands. Do not merge these rows with any warm measurement.

Same attributes, same binary, same host, same `-P 12`, the only difference
being the `eval-backend` setting:

| binary | tree | attrs | cpp | shadow | ratio |
| --- | --- | --- | --- | --- | --- |
| `720fafc8` | pinned 25.11 | 2638 | 69s | 1681s | **24.4x** |
| `720fafc8` | unstable | 2638 | 67s | 400s | **6.0x** |
| `114c6f00` | unstable | 2638 | 67s | 397s | 5.9x |
| `114c6f00` | unstable | 692 | 17s | 153s | 9.0x |

**The ratio is not a constant and three of these four rows are the same
backend**, which is the more useful finding than any single number. Read the
rows against each other:

- The two unstable 2638 rows, one per binary, are 5.9x and 6.0x. So the two
  fixes between the binaries cost essentially nothing, even though one of them
  (`filterSource`) turned 52 instant refusals into real work.
- The pinned row's 24.4x is almost entirely ENG-12913. Strip the 217
  attribute-not-found rows and the per-attribute cost is 1.63s against
  unstable's 1.48s.
- The 692 row is 9.0x on the same binary and tree as the 5.9x row. Same
  backend, same day, different attribute mix.

So quote the workload beside the ratio, or the ratio means nothing. A single
headline multiple for "what shadow costs" is not a thing this data supports.

Either way, leaving shadow on across a fleet is not free the way the corpus
figure (0.35s over 261 tiny expressions) suggests: that workload was too small
to show this.

The default `eval-shadow-budget` of 120s was set to 0 for these runs. Each
attribute is its own process so the budget would rarely bind, but a run that
hit it would convert its tail into `skipped[budget]`, and a report quietly
missing its tail is worse than a slow one. `skipped[budget]` is 0 everywhere.

## What this run did not do

- **Workload 3 was not attempted.** NixOS toplevel partial evals are E2E-1's
  frontier and it had not fallen during this window.
- **The ix flake's own outputs were not evaluated**, because `getFlake` and
  flake installables are both still refused. What ran is the inventory reached
  by direct import, which is real fleet data but is not `nix eval
  .#nixosConfigurations...`.
- **ix's `lib/` was not swept.** `lib/default.nix` requires `indexLib` from the
  index submodule, which a `git archive` snapshot does not carry. Reachable
  with a full checkout; not done here.
- **One host, one build, one architecture.** Nothing here ran on a production
  host, and no dev-compute host ships to ClickHouse (ENG-10011), so none of
  this is visible to the observability pipeline.
- **The harness reports ambient settings from the wrong binary.** It prints
  `pure-eval = true` because it asks `nix config show`, and `nix` defaults
  differently from `nix-instantiate`, which is what actually ran and which runs
  with `pure-eval = false`. The printed line is misleading about the measured
  command. Recorded rather than fixed silently; the sweeps genuinely ran
  impure, which is why store paths resolve.
- **The 27 `agreed-failure` and 63 `agreed-failure-text-differs` rows were
  counted, not enumerated.** Only the divergent ones were attributed
  individually.
- **ENG-12874 was found, not fixed.** Every number here carries its
  correction, but a future run on an unfixed binary will undercount again in a
  way only the `stats files read` line reveals. It did exactly that in run 2.
- **ENG-12913 was found, not fixed**, and it makes the pinned run's wall clock
  and Rust-arm totals unrepresentative of steady-state cost. The per-attribute
  figure excluding it (1.63s) is the one to carry forward.
- **The attribute list is generated from unstable**, which is why 217 of its
  entries do not exist in the pinned tree. Those rows are honest
  `agreed-failure` comparisons and were not dropped, but anyone wanting a
  pinned-native denominator should regenerate the list against the pinned
  tree. Not done here, because keeping one list makes the two trees
  comparable, which mattered more.
- **Workload 2 was not re-run at run 2.** The ix inventory numbers above are
  from `114c6f00`. Nothing in the two fixes touches that surface, but that is
  an inference and not a measurement.

## Reproducing

    # workload 1a
    maintainers/ix/shadow-nixpkgs-sweep.sh <bindir> maintainers/ix/shadow-nixpkgs-attrs-wide.txt \
      --nixpkgs <unstable-store-path> --jobs 12

    # workload 1b
    maintainers/ix/shadow-nixpkgs-sweep.sh <bindir> maintainers/ix/shadow-nixpkgs-attrs-wide.txt \
      --nixpkgs <pinned-store-path> --jobs 12
      # run 1 took `--expect-refusal-token path-interpolation`; that token no
      # longer exists and the arm no longer refuses, so reproducing run 2 means
      # dropping the flag rather than renaming it.

    # workload 2
    maintainers/ix/shadow-nixpkgs-sweep.sh <bindir> maintainers/ix/shadow-ix-inventory-attrs.txt \
      --nixpkgs <unstable-store-path> --root-file <ix-root>.nix --jobs 12

`shadow-nixpkgs-attrs.txt` is the narrower 692-attribute list, kept because
the numbers measured against it are quoted above and a reproduction needs the
exact list.

`--nixpkgs` is not optional in spirit. Omitting it falls back to the flake
registry and the harness warns on stderr that the run is not reproducible,
which is ENG-12855's lesson applied to this script.

## Tickets this run filed

- **ENG-12854** `stringLength` and `substring` reject a path cppnix coerces.
  **Blocks the flip.** Nine packages stop evaluating.
- **ENG-12852** interpolated path literals refused; one occurrence walls all
  of pinned nixpkgs 25.11.
- **ENG-12855** `nixpkgs-frontier.sh` ratchets against the floating registry,
  never against the fork's pinned nixpkgs.
- **ENG-12874** byte-truncating a divergence detail mid-UTF-8 destroys the
  whole process's census. It undercounted this report by 7 divergences in run
  1 and the same 7 in run 2. Deterministic, still unfixed at `77b95143b`.
- **ENG-12913** attribute-not-found on a large attrset costs 59s in the Rust
  arm against 0.2s in cpp. Not a correctness problem; it consumed 79% of the
  pinned run's Rust arm time and is why any overhead figure measured over a
  workload containing missing attributes is mostly measuring this.
