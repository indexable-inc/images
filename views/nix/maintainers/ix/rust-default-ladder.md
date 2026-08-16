# What has to be true before `eval-backend = rust` is the default

The owner's target is that the bytecode backend becomes opt-**out**: the fork
defaults to `eval-backend = rust`, a consumer that needs the old evaluator
sets `eval-backend = cpp`, and the rollback is that one setting rather than a
redeploy.

This file is the scoreboard for that, not a plan for it. Every rung states
what would have to be measured, and what the measurement says today. A rung
with no number under it is not "in progress", it is unmeasured, and saying so
is the point: the failure mode this file exists to prevent is a flip argued
from the rungs that happen to be green.

Status as of **63b79bf8d** (2026-08-05), measured on dev-compute-4 unless
stated. Re-measure before quoting; several of these move weekly.

---

## Rung 0: the setting has to reach more than one command

**Status: PARTIAL. `nix-instantiate --eval` and `nix eval` both serve it; every
other command refuses by name.**

`EvalState`'s constructor reads the setting now (#59), and
`EvalState::eval` -- the one place every user expression passes through --
refuses when the command reached it with `rust` selected. So a command that
does not implement the backend says so instead of quietly running the C++
one. That is the choke point; what changes per rung is how many commands get
past it rather than into it.

Two do:

| command | what it serves |
|---|---|
| `nix-instantiate --eval` | whole expressions, `-A` attribute paths, plain/`--json`/`--raw`/`--xml --no-location` |
| `nix eval` | `--expr` and `--file` sources, attribute paths, plain/`--json`/`--raw` |

Of the four refusals this rung opened with, one is left: `nix-instantiate`
without `--eval`, i.e. instantiation. `--xml` was a fifth and is now served
under `--no-location`; only the with-locations spelling still refuses,
because the rust document carries no source positions (ENG-12137). The `-A`
refusal is gone, which
mattered more than its count suggests: `lang-diff.sh` never passes `-A`, so
the corpus could have gone fully green with the most common real invocation
unsupported.

`nix build`, `nix-build`, `nix-env` and all flake evaluation still refuse.
Flake evaluation is the large one and is not close: it is fetching, locking
and `derivationStrict`, so it sits behind rung C.

**Gate:** every entry point that evaluates consults the setting (done), and
each refusal that remains is deliberate and written down (done for these two
commands; the others refuse generically through `requireBackendCanServe`,
which names the setting but not the feature).

## Rung A: the lang corpus agrees, and `unimplemented` counts as red

**Status: PARTIAL. 123 of 259 match, 37 unimplemented, 3 mismatch, and none
of the three is this backend's evaluator.**

Latest full run, arm A is the oracle, on dev-compute-4 at `14eef6543`:

```
RESULT lang-diff bin=.../build-rust/src/nix/nix-instantiate
sha256=32d717befd1d971967eac446704963e7f3a5874807876e55c24d5a3831558418
armA=eval-backend=cpp armB=eval-backend=rust
pairs=259 corpus=259 match=123 fail-as-fail=87 mismatch=3 crash=0
unimplemented=37 allowlisted=8 corpus-fail=0 skipped=1
```

**The three mismatches are cppnix lint flags, not evaluator divergences, and
they predate the change that made this line move.** `eval-fail-url-literal`,
`eval-fail-short-path-literal` and `eval-fail-abs-path-fatal` each pass a
`--lint-*-literals fatal` flag; cppnix refuses the file, this backend
evaluates it, because the lints are the fork's own parser diagnostics and rnix
has none of them. They were invisible until #85 fixed ENG-12438, which had
been scoring any pair with a `.flags` file as `unimplemented` without running
it. Measured rather than assumed: the tip with this rung's changes reverted
and rebuilt scores `match=122 mismatch=4`, the same three plus
`eval-okay-curpos`.

**`builtins.unsafeGetAttrPos` answered `null` at this rung**, by owner
decision (ENG-12591), carried by three `semantic-divergence` allowlist
entries. `eval-okay-getattrpos-undefined` **matched** rather than being
waived: it expects `null`, because `builtins`' own attributes have no position
in cppnix either.

**Superseded 2026-08-06 (ENG-12137).** Positions are in the IR, the three
allowlist entries are deleted, and all four getattrpos cases match. See
`maintainers/ix/positions.md`.

**One mismatch left, and it is `eval-okay-curpos`.** `__functor` closed the
other one: a set carrying that attribute is callable, cppnix rewrites
`set arg` into `set.__functor set arg` (`eval.cc:1880`), and without it every
callable attrset raised a type error. It came off the frontier probe rather
than off this list -- `stdenv.mkDerivation` is one, so it stopped the nixpkgs
package set dead -- which is the second time in two sittings that the corpus
and the package set disagreed about what was urgent.

`builtins.trace` (2 files) and `builtins.addErrorContext` came the same way.
The new allowlist entry is `eval-fail-addErrorContext-example`, tier
`trace-format`: both arms end with `error: kaboom` and produce no value, but
cppnix prints ten `… while counting down` frames and then truncates, which
drops the marker the harness reads a class from. Revisit it when ENG-12137
lands rather than keeping it.

The most recent move is search path lookup (ENG-12443), and it moved **one**
of the two files it was expected to. `eval-okay-redefine-builtin` matches.
`eval-okay-search-path` does not: its first line is `import
<nix/fetchurl.nix>`, which cppnix resolves through the in-memory `corepkgs`
accessor, and this evaluator reads paths off the real filesystem. The bridge
refuses that by name rather than handing back `/fetchurl.nix`, which is a path
that looks fine and does not exist. So the file stays `unimplemented` and the
refusal names the reason.

The six moves before that, all rung C: `eval-okay-context-introspection` (#78) and
`eval-okay-derivation-legacy` (#79) became matches, and four `eval-fail`
files that used to report `unimplemented` now fail with cppnix's class
(`eval-fail-addDrvOutputDependencies-*`, three of them, and
`eval-fail-derivation-structuredAttrs-stack-overflow`).

Over the 150 `eval-okay` files alone: 141 compile, 115 evaluate.

Two things about this number that are easy to misread:

**`unimplemented` is not a pass.** It is 50 of 259 pairs, and `lang-diff.sh`
exits 0 on them. A flip decision that reads "0 mismatches" off this line is
reading past 50 files the backend cannot do. The gate is `unimplemented=0`,
not `mismatch=0`.

**`unimplemented` currently also absorbs cases the harness could not run.**
`eval-fail-infinite-recursion-lambda` scored `unimplemented` not because the
expression is unsupported but because its `.flags` file replaces the default
flags, dropping `--eval`, so the bridge refused before evaluating anything
(ENG-12438). Any pair with a `.flags` file is suspect until that is fixed.

The 2 remaining mismatches are the honest failures: `eval-okay-callable-attrs`
(`__functor`) and `eval-okay-curpos` (`__curPos`).

The 4 allowlisted divergences are in `eval-allowlist.toml`, three
`error-text` and one `semantic-approved`.

**Three of the five mismatches this line used to carry were one mechanism, and
it is the one worth reading about even now that it is fixed.** A path
interpolated into a string is coerced by cppnix with `copyToStore = true`, so
it copies the file into the store and returns the store path with context
(`eval.cc:2582`), and the missing-path error falls out of `copyPathToStore`.
This backend returned the bare source path.

The corpus saw only the missing-path half, in
`eval-fail-bad-string-interpolation-2` and `eval-fail-nonexist-path`. For a
path that exists the rust arm returned a value, successfully, and the value
was wrong. Nothing caught it, because the corpus runs with
`NIX_REMOTE=dummy://` and its only path interpolations are those two
missing-path cases. A silent wrong answer is a different and worse thing than
an `unimplemented`, and that one was invisible to this scoreboard.

The cheap fix -- check existence, error -- would have moved mismatch from 5 to
3 while leaving the wrong value in place, buying a greener number by deleting
the only signal that anything was broken. The coercion is done properly
instead (ENG-12447): the copy leaves the VM through `Host` as a
`NeedPath::StorePath`, the bridge answers it with cppnix's own
`copyPathToStore`, and the two arms now agree byte for byte in both read-only
and read-write mode. `tests/functional/rust-eval-path-to-store.sh` is the test
that can see it, because it runs against the test's own real store rather than
`dummy://`.

What moved: mismatch 5 to 2, fail-as-fail 77 to 79 (the two missing-path cases
now fail on both arms with identical text), and `eval-okay-context` went to
match once contexts landed beside it (ENG-12465), taking match from 111 to 112.

The 2 that remain are the honest ones, and neither is about paths.

**Gate:** `mismatch=0 crash=0 corpus-fail=0 unimplemented=0`, with every
allowlist entry still carrying a reason and a human name where the tier
demands one, and ENG-12438 fixed so no pair is exempt by accident.

---

## Rung B: nothing nonterminates, and eval-fail fails bounded

**Status: DONE for the known class, with one gap named below.**

`(x: x x) (x: x x)` reached 67 GB before being killed (ENG-12432). It now
fails in **0.14 s and 8 MB RSS** with cppnix's own wording, `stack overflow;
max-call-depth exceeded`, and `--max-call-depth` reaches the VM so a corpus
case passing the flag exercises the same limit on both arms.

The three deep-structure cases fail bounded too: `eval-fail-*stack-overflow*`
scores `fail-as-fail=2 unimplemented=1 mismatch=0`.

**The named gap:** the limit counts closure applications. A builtin that
recurses internally over a deep value (`deepSeq`, `toJSON`, structured-attrs
serialisation) is not covered by it. Structured-attrs serialisation has since
been implemented (#79) and reuses `builtins.toJSON`'s own walk, so it
inherits whatever that walk's depth accounting is, which is nothing; the
corpus case for it, `eval-fail-derivation-structuredAttrs-stack-overflow`,
scores `fail-as-fail` because both arms die, not because either bounds
itself. If any of these grows a deeper shape without its own accounting, it
is unbounded again. Worth a guard before the flip, not after.

**The second gap is closed, and closing it is a deliberate divergence from
cppnix.** ENG-12524, ENG-12533, landed in `910968dfe`.

`Vm::poll` checks an interrupt flag every 2048 iterations. The flag is the
embedder's -- `nix::isInterrupted()`, an atomic load, through
the session vtable's `interrupted` -- and the failure is cppnix's own wording,
`interrupted by the user`, uncatchable by `tryEval`, because an operator's
SIGTERM must not become a value.

It is **not** a `Host` question, unlike every other way this evaluator
reaches outside itself, and that is the design point worth keeping. A `Host`
question is part of what the expression means and is recorded in a read set;
an interrupt is a fact about the process, and the case that needs it is
precisely a computation that never returns to the scheduler.

Measured on dev-compute-4, SIGTERM at 5s into a 45,000-wide `hashString`
expression, `maintainers/ix/sigterm-gate.sh`:

```
RESULT sigterm-gate rust_rc=124 rust_elapsed=5s bound=15s cpp=[rc=137 elapsed=20s]
```

The rust arm dies on the signal. The cpp arm does not and is SIGKILLed at 20s,
because cppnix checks no interrupt while evaluating -- `rg checkInterrupt
src/libexpr` finds five sites, none in `eval.cc` -- and notices only at the
first checkpoint after the evaluation, which on this path is printing. The
36s in this file's earlier measurement was how long that evaluation took, not
how long the signal took to arrive. So the rust arm is now strictly *more*
responsive than cppnix, which is the divergence: nothing in the corpus runs
long enough to see it, and an operator who can kill a runaway is better served
than one who cannot.

Watched failing: with the check removed the gate reports `rust_rc=137
rust_elapsed=20s`, SIGKILL, and the crate test `an_armed_interrupt_stops_a_
long_pure_evaluation` fails with it.

**What it does not cover**, which is rung B's existing gap in a new place: the
stride bounds `poll` iterations, and a unit's op count is bounded by its
source, so the only thing that can outrun it is a single builtin step that is
itself unbounded -- `genList` with a huge count, or the recursing builtins
above. Those are still uninterruptible.

**Gate:** every `eval-fail` file bounded in time and memory, and a deliberate
adversarial pass over the recursing builtins rather than only the corpus.

---

## Rung C: derivation hash parity

**Status: STEP 1 MET, and all six derivation-shaped corpus files match.
Steps 2 and 3 are not met. Still the schedule risk.**

An `outPath` computed by the *evaluator* is byte-identical to the cpp
backend's, which is what step 1 asks for. `builtins.derivationStrict` exists,
and 40 of 60 derivation-shaped expressions run through one binary under both
backends agree byte for byte, with **`mismatch=0`** (§ "From an expression, at
last" below).

Since then the `derivation` global is bound (PR #75), so a derivation can be
written the way anyone actually writes one, and printed. On dev-compute-4 both
backends of one binary print the same 343 bytes for
`derivation { name = "w"; system = "x86_64-linux"; builder = "/bin/sh"; }`,
`«repeated»` markers included.

**All six corpus files this rung named now match.** Four moved with the
`derivation` global (`eval-okay-eq-derivations`, `eval-okay-delayed-with`,
`eval-okay-delayed-with-inherit`, `eval-okay-substring-context`), then
`eval-okay-context-introspection` with `builtins.appendContext` (#78,
ENG-12479) and `eval-okay-derivation-legacy` with `__structuredAttrs` (#79).
lang-diff went from `match=112 unimplemented=61` to `match=118
unimplemented=50`, with `mismatch` unchanged at the same pre-existing 2
(`eval-okay-callable-attrs`, `eval-okay-curpos`).

Two things that landed with those and are worth knowing about separately:

* **`appendContext` needed the store, and only for half of what it asks.**
  cppnix validates every key with `store->isStorePath` and then calls
  `store->ensurePath` (`context.cc:270`). The first is a pure function of the
  store directory, which the embedder already hands over, so it is
  `storepath.rs` and not a hook. The second is a real store operation, and it
  is skipped entirely under `readOnlyMode`, so it leaves through
  `Host::ensure_path` with the `readOnlyMode` branch on the C++ side where
  the setting lives.
* **The evaluator can warn now.** cppnix warns about six attributes
  `__structuredAttrs` silently disables, and the crate had nowhere to put a
  warning. `Host::warn` / `NeedPath::Warn` / the vtable's `warn` is the same shape
  as the store hooks, with one difference: a warning is an output rather than
  a question, so it is answered before the `restrict-eval`/`pure-eval` access
  check. Refusing it there would let a purity setting change what a program
  means. With it, both arms produce `eval-okay-derivation-legacy.err.exp`
  byte for byte.

Three things had to land together, and the order is worth keeping because each
was only visible once the one before it was fixed:

1. **The printer had no `«repeated»`** (ENG-12517, PR #73). The wrapper's result
   contains itself through `all`, so printing a derivation did not terminate.
   Both dialects, not just one.
2. **Attribute sets did not coerce** through `__toString` or `outPath`
   (ENG-12513, PR #74). Without it `"${drv}"` raised a type error, which turned
   `eval-okay-substring-context` from an `unimplemented` into a *mismatch* the
   moment `derivation` was bound.
3. **Two derivations compared structurally** (PR #75, first commit). cppnix's
   `eqValues` compares them by `outPath` alone. That is semantic and not an
   optimisation: `drvA1 == (drvA1 // { dummy = 1; })` is true although the sets
   differ in size. It is also what makes the comparison terminate, and without
   it `eval-okay-eq-derivations` ran over ten minutes at 99% CPU.

Read the rest of this rung as unmet. Step 2 is the `.drv` *bytes*, which
nothing has compared yet because nothing writes one (`addTextToStore`,
ENG-12491); step 3 is 500 real packages. A flip argued from "derivations
evaluate now" would be reading this backwards: what this buys is that the
arithmetic below is reachable, demonstrably right, and now exercised by four
corpus files, not that building anything works.

One piece of it does exist now. ENG-12447 put a store behind the evaluator:
`Host::copy_to_store` is the boundary, the session vtable's `copy_to_store`
hands the embedder's implementation over, and the bridge answers out of the
live `EvalState`. Everything below needs the same shape -- writing a `.drv` into
the store is `addTextToStore` through that same hook -- so the question of
where the store lives is settled and does not have to be reopened per step.

The brick that is missing before step 1 can even start is **string contexts**
(ENG-12465). `derivationStrictInternal` reads `inputSrcs` and `inputDrvs` off
the contexts of the attribute values it is handed, so a backend whose strings
carry no context cannot produce a matching `.drv` however good the rest is.

That brick is in place (ENG-12465). `Value::Str` is a `NixStr` carrying an
optional set of `ContextElem` (`Opaque` / `DrvDeep` / `Built`, cppnix's three
cases), and every string builtin this crate implements has a verdict taken out
of cppnix's primops one at a time rather than guessed:

| builtin | cppnix | this crate |
|---|---|---|
| interpolation, `+` | `ExprConcatStrings` copies each part's | unions them |
| `toString`, `baseNameOf`, `dirOf` | `coerceToString`, context copied | propagates |
| `substring` | context copied; `len == 0` is the idiom for capturing a context with no bytes | propagates, empty case included |
| `concatStringsSep` | context of separator and every element | propagates the union |
| `replaceStrings` | context of `s` and of the replacements actually used, and NOT of the `from` strings | the same, including the omission |
| `toJSON` | accumulated across the whole value | propagates |
| `stringLength` | coerces with context, returns an int | nothing to carry |
| `match`, `split` | pattern is `forceStringNoCtx`, subject is not, captures carry none | refuses on the pattern only |
| `hashString` (both arguments), `splitVersion`, `fromJSON`, `fromTOML`, `getAttr`, `hasAttr`, `removeAttrs`, `listToAttrs` names, `catAttrs`, `groupBy`'s key, `getEnv`, `compareVersions` | `forceStringNoCtx` | refuses, with cppnix's wording |
| a dynamic attribute name -- `set.${e}`, `set ? ${e}`, `{ ${e} = v; }` | `getName` (`eval.cc:247`) and `eval.cc:1434`, both `forceStringNoCtx` | refuses; not a builtin, and a backend that covered only builtins would still disagree |
| `readFile` | context is the store paths occurring **in the file's bytes**, rescanned | **returns none; the one verdict not implemented** |

The list is enumerated from `rg forceStringNoCtx src/libexpr` rather than from
the builtins that came to mind, which is how the last three rows were found:
`catAttrs`, `groupBy` and dynamic attribute names had no verdict at all, and
each one silently produced a store path where cppnix raises an error.

`builtins.getContext`, `hasContext` and `unsafeDiscardStringContext` are
implemented on top of that, which is what took `eval-okay-context` to match.

One gap remains in this brick, named rather than silent:

* **`readFile`** (ENG-12478) hands back no context where cppnix scans the
  file's bytes for store paths. A derivation whose attribute is a `readFile`
  of something in the store would under-depend. It needs a store-side scan, so
  it is the same shape as the ENG-12447 hook rather than a local fix.
`appendContext`, `addDrvOutputDependencies` and
`unsafeDiscardOutputDependency` were the other gap and are done (#78,
ENG-12479); `eval-okay-context-introspection` matches. So `readFile` is the
one verdict in the table above still unimplemented.

### The ATerm form, and what its round trip does and does not prove

The ATerm form of a derivation -- cppnix's `Derivation::unparse` and a parser
for it -- is in `drv.rs`, and byte-exactness is established against real stores
rather than fixtures. Re-measured on **dev-compute-4** at the head of this
section's revision, over every `.drv` in that box's store:

```
RESULT drv-roundtrip files=45702 ok=45702 differs=0 unordered=0 dynamic=0 errors=0
```

The earlier runs stand as separate denominators on separate stores:
`files=355304` on aarch64-darwin and `files=65527` on dev-compute-2, both
`differs=0`, though neither was checked for ordering, which did not exist yet.

**A round trip alone is a weaker claim than it reads as, and #65 was merged
with it as the whole story.** `parse` keeps whatever order it found on disk and
`unparse` emits that order back, so the pair agrees under *any* ordering rule,
including a wrong one. A derivation built by `derivationStrict` has no disk
order to inherit and has to produce cppnix's, which comes from the container
types in `BasicDerivation` -- `std::map` for `outputs`, `inputDrvs` and `env`,
`StorePathSet` for `inputSrcs`, and a plain vector for `args`, which is
therefore *not* sorted. `drv::canonicalise` is that rule and `unordered=0`
above is it agreeing with 45,702 files cppnix wrote.

The census is the other half of reading that number honestly. "45,702
derivations agreed" and the same sentence with the shapes named are different
claims:

```
RESULT drv-census files=45702 outputs-input-addressed=31030 outputs-ca-fixed=16347
  outputs-ca-floating=5900 outputs-deferred=424 outputs-impure=0 outputs-unrecognised=0
  files-multi-output=5403 files-structured-attrs=17777 files-non-ascii=1237
  files-with-escapes=42670 files-no-inputs=197
```

Four of the five output kinds, 5,403 multi-output derivations, 17,777 carrying
structured attributes and 1,237 with non-ASCII bytes. **`outputs-impure=0` and
`dynamic=0`** are the holes: no real derivation in this store is impure or a
`DrvWithVersion`, so both are covered by a unit test and by nothing else.

### Output paths: `hashDerivationModulo` and `makeOutputPath`

`drvpath.rs` computes where a derivation's outputs land: `nix32_encode`,
`compressHash`, `makeStorePath`, `outputPathName`, `makeOutputPath` and
`hashDerivationModulo`, each transcribed from the named cppnix function.

The oracle is cppnix's own `Derivation::checkInvariants`
(`src/libstore/derivations.cc:1398`), which asserts that an input-addressed
output's path equals `makeOutputPath(name, hashDerivationModulo(drv,
true).hashes[name], drvName)`. Every `.drv` in a real store already passed it
once, so recomputing the path from a file's own bytes and comparing is a check
with a store-sized denominator and no evaluator involved:

```
RESULT drv-outpath store=/nix/store files=45702 agrees=23031 outputs-agreed=31030
  differs=0 not-input-addressed=16347 deferred=6324 errors=0 input-drvs-read=82912
```

**31,030 output paths recomputed from scratch, all byte-identical**, over
23,031 derivations, on dev-compute-4. That exercises the ATerm writer, output
masking, the modulo recursion through input derivations, `compressHash` and the
base-32 alphabet together. `hello` is one of them, named because it is the
line in this file it answers:

```
/nix/store/0nnrl637vw6ibnjym17l3s0yzj5zr77n-hello-2.12.3.drv  agrees  1 output(s)
```

`not-input-addressed=16347` and `deferred=6324` are counted apart from
`agrees` on purpose: cppnix computes no input-addressed path for those either,
so folding them into a pass would let a corpus of nothing but fixed-output
derivations report zero mismatches while proving nothing.

The `.drv`'s own path is a **different** computation and is checked separately,
because it can be wrong on its own. A derivation is stored as text, so its path
is `makeStorePath` with the type string `text` followed by every reference,
where the references are `inputSrcs` plus every `inputDrvs` key and
deliberately not the outputs (`infoForDerivation`, `derivations.cc:109`). Get
that set wrong and the `.drv` path moves while every output path stays right,
which is exactly the latent divergence step 2 is about:

```
RESULT drv-selfpath store=/nix/store files=45702 agrees=45702 differs=0
```

Every derivation in the store, its own path recomputed from its own bytes.
Against `--store /nix/stor2` the same 200-file sample goes to `differs=200`, so
the check is discriminating rather than passing by construction.

### Building one, rather than reading one

Everything above reads a derivation and checks a number about it.
`drvpath::build_input_addressed` goes the other way: given a name, a platform,
a builder, its arguments, its output names, its environment and its inputs, it
produces the finished derivation the way `derivationStrict` will have to. That
is `derivationStrictInternal` from the point where the attributes are forced,
minus the content-addressed branches, and it is the last piece of step 1 that
is a pure function of strings.

`drvpath::inputs_of` is the inverse, so every input-addressed derivation in a
store is a test case for it: take it apart, build it back, require the bytes
and the `.drv` path to match.

```
RESULT drv-rebuild store=/nix/store files=45702 agrees=23031 differs=0 not-input-addressed=22671
```

**23,031 derivations reconstructed from their parts**, each byte-identical and
landing at the path cppnix gave it. This is the direction a round trip cannot
test at all: the builder is handed no disk order and no output paths, and has
to produce both.

The order of operations inside it is cppnix's and every step of it matters.
Each output gets an *empty* environment variable and a `Deferred` entry first,
so the set of output names is in the hash even though no path is; the modulo
hash is taken over that; each output path is then written into two places, the
output entry and the variable named after it; and only then is the `.drv`
rendered and its own path computed, over the filled-in form.

Watched failing, on exactly that ordering. Moving the `.drv` path computation
before the output paths are filled in, which is self-consistent and
reproducible and not cppnix, turns a 300-file sample from `agrees=225
differs=0` into `agrees=0 differs=225`; restoring the order restores the
result. `drv-selfpath` stayed at `agrees=300` throughout that break, which is
why the two are separate checks.

Three claims in `drv.rs` were wrong and are corrected (ENG-12498). The worst
was the comment on `unparse`, which said masking "is the form
`hashDerivationModulo` hashes". It is not, and the difference is every
derivation with an input: `hashDerivationModulo` also substitutes each input
derivation's path with that input's own modulo hash (which re-sorts the list
into hash order), short-circuits fixed-output derivations away from `unparse`
entirely, and recurses **unmasked**. An implementation written from that
comment would produce a wrong `outPath` and the symptom would point at the hash
function.

The three guards were each watched failing on real bytes, since a guard nobody
has broken is not a guard: appending one byte to `hello-2.12.3.drv` now reports
`malformed derivation at byte 1861: expected end of input`; permuting two `env`
entries reports `unordered`; and running `drv-outpath --store /nix/stor2` turns
`agrees=153` into `differs=153`.

### From an expression, at last

`builtins.derivationStrict` (`rust/nix-eval-rs/src/drvstrict.rs`) is the part
that needed a VM: force the attribute set, walk it in cppnix's
`lexicographicOrder`, coerce each value to a string with its context, read
`outputs` and `__ignoreNulls`, and turn the accumulated context into the
`inputSrcs` and `inputDrvs` that `build_input_addressed` takes.

Measured on dev-compute-4, one `nix-instantiate` binary, 35 derivation-shaped
expressions under `eval-backend=cpp` and `eval-backend=rust`, compared on
stdout and exit code:

```
RESULT cases=60 match=40 mismatch=0 unimplemented=3 both-fail-differently=17
```

Differential rather than golden, for the reason the other gates give. The 17
`both-fail-differently` are cases both arms reject with different message
text, which is lang-diff's `fail-as-fail`; the 3 `unimplemented` are the
refusals listed below. **`mismatch=0` is the gate.** The harness is
[`drv-parity.sh`](./drv-parity.sh); the same four shapes are pinned as goldens
in the crate's own tests, taken from cppnix:

| expression | `drvPath` |
|---|---|
| literal | `x0sj6ynccvc1a8kxr8fifnlf7qlxw6hd-hello.drv` |
| one input derivation | `g2wnwbbdkb6ww7124j7y0a2zhrfxs714-b.drv` |
| two deep, plus an `args` dependency | `v15m7i45c5ihs7x3637463dfl8xmpk8r-c.drv` |
| two outputs | `vp4yhflnpb33asgsyxpxgai5qhrap4qk-multi.drv` |

The chained rows are the ones that carry weight. A leaf derivation's own path
does not depend on `mask_outputs` at the memo insert, so the literal row passes
with that flag set either way, and a test asserting only that a consumer's path
*moved* passes for a wrong input hash too. Watched failing: flipping that flag
turns both chained rows red and leaves the literal one green.

**Two float renderings, and the one that is hashed.** The cross-backend run
found one real mismatch on its first pass, and it had been latent since long
before derivations. cppnix prints a float with `%.6g` and *coerces* one with
`std::to_string`, which is `%f`; the crate had only the printer's, so
`builtins.toString 1.5` answered `"1.5"` where cppnix answers `"1.500000"`.
Nothing in the corpus coerces a float, so it was invisible until a float became
a derivation attribute and moved the `outPath`:

```
cpp  /nix/store/9rw8ddck9f6gq4chbk80iaifrc15hxkk-g.drv
rust /nix/store/w1rjby9dxn67nrmxfj1yiik4ndvr2ifh-g.drv
```

Well-formed, reproducible, and not cppnix's -- the failure shape this whole
rung exists to catch, found by the differential and by nothing else.

**Refused by name, never answered wrong:**
`__contentAddressed`, `__impure`, `outputHash`/`outputHashAlgo`/
`outputHashMode` (fixed-output paths are `makeFixedOutputPath`, which `drvpath`
can recompute from a store but not construct), `__json`, a dependency on every
output of a derivation (cppnix answers that with `computeFSClosure`, a store
read of the whole graph), a `.drv` that this evaluation did not produce, and a
store directory the embedder never supplied.

**The store directory is now handed over rather than assumed.** It goes inside
the fingerprint `makeStorePath` hashes, so it is an input to every path and not
a prefix on one; `RustEvalSetup` passes `state.store->storeDir` across the ABI
and the primop refuses without it.

### What is still missing

Two things this file has been listing as prerequisites turned out not to be,
and knowing that is what made step 1 a day's work rather than a week's.

**The modulo recursion does not have to reach the store.**
`hashDerivationModulo` recurses into every input derivation, which reads other
`.drv` files, and in a VM whose only IO is a `NeedPath` suspension that reads
like a resumable read-driven recursion inside a builtin. It is not, because
`pathDerivationModulo` (`derivations.cc:861`) consults the process-global
`drvHashes` first, and `derivationStrictInternal` populates it for every
derivation it produces (`primops.cc:1939`). Its comment says why in as many
words: *"Optimisation, but required in read-only mode! because in that case we
don't actually write store derivations, so we can't read them later."* Every
input of a derivation built during an evaluation was itself built earlier in
that same evaluation, so the hash is in memory. The Rust side needs the same
in-process map and can **refuse by name** on a miss, which is the case where a
`.drv` arrives from outside the evaluation (`builtins.storePath`, an imported
derivation path). That is a named gap rather than a store read on the critical
path.

**A `.drv` does not have to be written to get a `drvPath`.** Under
`readOnlyMode`, which `nix-instantiate --eval` sets, cppnix calls
`computeStorePath` instead of `writeDerivation` (`primops.cc:1917`) and no
bytes move. `computeStorePath` is `drvpath::derivation_store_path`, measured
above at 45,702 of 45,702. So the `addTextToStore` hook (ENG-12491) is what
`nix-build` needs, not what step 1 needs; step 1 can be reached read-only and
the hook can follow.

Sequence, each step a real gate:

1. **MET.** An `outPath` byte-identical to the cpp backend, computed from an
   expression. `mismatch=0` over 35 shapes, above.
2. The `.drv` files themselves byte-identical, not only the output paths. A
   matching `outPath` with a differing `.drv` is a latent divergence. Blocked
   on `addTextToStore` (ENG-12491): nothing writes a `.drv`, so there are no
   bytes to compare. Note that step 1 passing says less here than it looks --
   the `.drv` path is a hash *of* those bytes, so agreement is strong evidence
   and not a comparison.
3. A curated 500-package set, byte-identical `.drv` and `outPath`. Blocked on
   two things before the packages: the `derivation` wrapper, and ENG-12513 --
   an attribute set does not coerce through `outPath` or `__toString` here,
   which is how `buildInputs = [ pkg ]` works and is a mismatch rather than a
   gap.

**Gate:** step 3 green, reported with its denominator and the exact nixpkgs
revision.

---

## Rung D: the ix repo's own host evaluations are byte-identical

**Status: NOT STARTED, and blocked behind rung 0.**

Every host's `config.system.build.toplevel.drvPath` under both backends,
compared byte for byte. This is the workload that actually matters here, and
it is the one the fork's consumers will hit first.

It is blocked on rung C, because a host closure is derivations all the way
down. It used to be blocked on rung 0 as well; `nix eval` now serves the
setting (rung H), but the flake half does not, and a host evaluation reaches
the package set through a flake.

**Gate:** all hosts, byte-identical `drvPath`, on a stated ix revision.

---

## Rung E: performance at parity or better, both numbers stated

**Status: STILL UNMEASURED ON A REAL WORKLOAD, and no longer unmeasured on
anything. One synthetic expression: 3.0x slower than cppnix, flat in N, at
1/27th the peak memory.**

The gate wants nixpkgs and ix host evaluation, and neither can run yet (rung
D is blocked on rung C, and the package set stops at `addErrorContext`; see
the frontier section). What exists is the one expression this file has
carried as a warning, now measured properly at both ends.

The expression, unchanged:

```
builtins.foldl' (a: b: a + b) 0 (builtins.genList (x:
  builtins.foldl' (p: q: p + q) 0 (builtins.genList (y:
    builtins.stringLength (builtins.hashString "sha512" (toString (x * 1000 + y)))) 1000)) N)
```

dev-compute-4, one `nix-instantiate` binary run under both backends, wall
clock and peak RSS from `time -f`. "before" is `8d6f5e52ee9e6499`, the binary
at `855e9b4d5`; "after" is `8650429590508a16`, the same tree plus the one
commit below:

| N | cpp | rust before | rust after | before | after | cpp RSS | rust RSS |
|---|---|---|---|---|---|---|---|
| 50 | 0.05s | 1.90s | 0.15s | 38x | 3.0x | 55 MB | 41 MB |
| 200 | 0.17s | 7.68s | 0.52s | 45x | 2.9x | 105 MB | 40 MB |
| 800 | 0.67s | 31.51s | 2.04s | 47x | 3.0x | 304 MB | 41 MB |
| 3200 | 2.58s | 122.62s | 8.13s | 47.5x | 3.04x | 1,103 MB | 41 MB |

Three things to read off it.

**The old line in this file understated the problem.** "36s cpp vs >125s
rust" reads as though the rust arm was a little over 125s at N=45000. It was
not: at N=3200 it was already 122.6s, so N=45000 extrapolates to about 1,700s.
The 125s was the timeout, not a measurement, and a number whose true value is
14x larger is the kind of thing a ">" hides.

**One mechanism was 15x of it.** `Op::BuiltinsSet` rebuilt the whole
`builtins` attrset on every `builtins.x` *evaluated*: ~200 interns, ~150
`format!` allocations for the unimplemented names, and one blake3 over the
entire `derivation` wrapper source, because the set binds
`builtins.derivation` and `derivation_cell` goes through `import_module`,
which keys on the text. An inner loop mentioning two builtins paid all of it
twice per iteration. The profile said so plainly -- `builtins_set` 11.67%,
the BTreeMap insert under it 11.64%, `Slot::drop` 10.77%, blake3 13.32%,
malloc and free about 25% -- and the fix is to build both once per VM, which
is what cppnix does anyway (`staticBaseEnv`, once per `EvalState`). ENG-12539;
the `Op::GetLocal` half of that ticket is somebody else's and is not in this
number.

**The ratio does not move with N**, so nothing superlinear is left in this
expression: not the O(n^2) sort stopgap, not the BTreeMap attrsets, at least
not here. That is a claim about one shape and not about a package set.

**Peak RSS is the column that favours this backend**, and it is the half a
wall-clock-only comparison would have missed: flat at ~41 MB across a 64x
range where cppnix reaches 1.1 GB, because cppnix's arena retains the whole
intermediate list and the VM does not.

What this still does not say: anything about a real package set, anything
about compile time versus evaluation time on a large file set, anything about
a second run with the memo table warm. The harness is
`maintainers/ix/eval-bench.sh` for the general case; the numbers above came
from a scaling script kept out of the repo because it is three lines of
`time` around one expression.

**Gate:** wall clock and peak RSS for both backends on the rung C and rung D
workloads, both numbers written down, rust at parity or better. A regression
that is understood and accepted is fine, but it has to be stated rather than
omitted.

---

## Rung F: incrementality gates stay green

**Status: GREEN, and the only rung with a standing automated check.**

`maintainers/ix/rust-incremental-gate.sh` reports `RESULT: pass` at fe7904a9d,
four arms each with a denominator: 150 files compared and agreeing for the
module round trip, 300 compared / 300 agreeing with `served_from_memo=147`,
`compile_hits=141`, 11 compared for the cross-process arm, and both edit cases
returning `1,1,2,2`.

The edit case is the one to keep. A memo keyed on the questions rather than
the answers passes the corpus arm with `agree=300 differ=0` and is still
wrong; only the edit case catches it.

**Gate:** this stays green, and cross-process persistence keeps its own arm as
it lands.

**Both build configurations are gates, not one.** `rust-eval` is a meson
option defaulting to `disabled`, so `-Dnix:rust-eval=enabled` and the default
are two different programs that diverge at every `#if`, and a change green in
one can fail to build in the other. `ix-patched`'s default build did not link
for the whole of rung H and nobody noticed, because everyone working on this
track builds with the backend on (ENG-12495). Both commands are in
[testing.md](./testing.md); run both.

---

## Rung G: the flip itself, and a rollback that is one setting

**Status: NOT STARTED.**

Mechanically the flip is changing the default in `eval-settings.hh` and
dropping `rust-eval` from the experimental features, so a consumer opts out
with `eval-backend = cpp`. What makes it safe is not the diff:

- **Rollback has to be one setting and no rebuild.** `eval-backend = cpp` in
  `nix.conf` restores the old evaluator, and that must be tested by doing it,
  not by reading the code.
- **The consumer list has to be enumerated first.** Every repo pinning this
  fork's rev is a consumer, and each needs to know the opt-out exists before
  the default moves rather than after.
- **`NIX_SHOW_STATS`'s `evaluator` field is the check.** It reports which
  backend actually ran, which is the only way to tell a flip that took effect
  from one that silently did not, and the only way for a consumer to confirm
  their opt-out worked.

**Gate:** default flipped, opt-out documented and exercised on a real
consumer, rollback performed once on a dev node and recorded.

---

## Rung H: values cross the C ABI, so a command can select and format

**Status: DONE for `nix eval` and `nix-instantiate`, measured below.**

The C ABI used to take source and return one rendered string, which can
answer "print this whole expression" and no other question. `nix eval` asks
three more -- select an attribute path, choose an output format, and do the
first without forcing what it did not select -- so the value itself has to
cross. It crosses as a handle: an opaque integer naming a lazy cell in one
session's table (`rust/nix-eval-rs/src/capi.rs`, ownership rules in
`ixe.h` beside the typedef).

Laziness is the load-bearing property and the corpus cannot see it, because no
corpus case selects an attribute. Two guards cover it, one in the crate and
one through the real binary: selecting `ok` out of
`{ ok = 1; boom = throw "..."; }` must print `1`, and enumerating that set's
names must not fire the throw either.

Rendering stays on the Rust side. Plain, `--json` and `--raw` all already
existed there and are compared against cppnix by the corpus; rendering from
C++ would have been a second implementation of each, since cppnix's own
printers take a cppnix `Value` and there is no such thing here. `--json` is
re-dumped through nlohmann on the C++ side so `--pretty` has one formatter.

**Measured**, `maintainers/ix/rust-nix-eval-gate.sh` on dev-compute-6 at
ac818271e, every case run twice through one binary and compared byte for byte:

```
RESULT rust-nix-eval-gate pairs=65 match=62 mismatch=0 served=51 refused=3
       produced=27 empty=0 lazy_ok=1 refusals_ok=1
```

`served` is pairs that agreed *and* produced a value; `produced` is the subset
the gate requires to be non-empty; `empty` is pairs that agreed by both
printing nothing. That last counter exists because an earlier revision of the
gate pinned `pure-eval` in its config, which made `--file` refuse in both arms,
and four cases scored as matches while nothing was being compared.

The three refusals are `nix eval` of a function: cppnix prints
`«lambda @ «string»:1:1»` and this IR carries no positions (ENG-12137).

Alongside, unchanged: `rust-incremental-gate.sh` `RESULT: pass`,
`rust-eval-cache-cli.sh` `ALL CLI CHECKS PASSED` with warm 6.4x cold, and 115
crate tests.

lang-diff is unchanged to the digit, and that was measured rather than assumed:
the base 6c338b28 was built separately and scored `match=111 fail-as-fail=79
mismatch=2 unimplemented=62`, identical to this branch. The improvement over
the figures this file used to carry is ENG-12447's and ENG-12465's, not this
rung's -- worth saying, because a branch that merely inherits a better number
and reports it as its own is how a scoreboard stops meaning anything. The
corpus never passes `-A` and never selects an attribute, so it could not have
seen this rung either way.

**What it does not do.** The handle path gets the compile cache but not result
memoisation: the memo stores rendered text keyed on the questions an
evaluation asked, and a value is not rendered text (ENG-12470). And the
evaluator reads the filesystem outside cppnix's access control, so under
`restrict-eval` or `pure-eval` it refuses to look rather than looking more
freely than cppnix would (ENG-12480).

**Gate:** the differential gate stays at `mismatch=0 empty=0` with `served`
rising as refusals are retired.

## Where the frontier actually is

Useful for sequencing, because the corpus and real nixpkgs disagree about what
is on the critical path.

`maintainers/ix/nixpkgs-frontier.sh` is this section, run rather than
remembered. For the evaluator half of the same question without a `nix` build,
`cargo run --release --example nixpkgs-probe` in `rust/` asks the same twelve
expressions in 3.5s on any machine; it is single-arm and skips the C++ bridge,
so it bisects and the script above decides (`maintainers/ix/testing.md`). Against nixpkgs `llgwlxshmy0if` (26.11pre-git), both arms of one
binary at `14eef6543`:

```
1  the lookup itself                  AGREE    "path"
2  lib alone                          AGREE    "26.11pre-git"
3  lib attr count                     AGREE    494
4  lib.strings                        AGREE    "ABC"
5  the top-level function             AGREE    "lambda"
6  the package set                    REFUSED  builtins.unsafeGetAttrPos
7  one package name                   REFUSED  builtins.unsafeGetAttrPos
8  one package outPath                REFUSED  builtins.unsafeGetAttrPos
9  stdenv                             REFUSED  builtins.unsafeGetAttrPos
10 currentSystem                      AGREE    "x86_64-linux"
11 a small package set                REFUSED  builtins.unsafeGetAttrPos
12 package set attr count             REFUSED  builtins.unsafeGetAttrPos

RESULT nixpkgs-frontier rows=12 agree=6 differ=0 refused=6
```

`differ=0` is the invariant: everything the backend reaches, it gets right,
and everything else refuses by name.

### The wall is down, and the milestone is one refusal away

At `63b79bf8d`:

```
RESULT nixpkgs-frontier rows=12 agree=11 differ=0 refused=1
```

Eleven of twelve rows agree, including `hello.name`, `stdenv.name` and the
27,682-attribute count of the whole top-level set. **`differ=0`, and the one
row left is a named refusal**: `builtins.derivationStrict with outputHash`,
which is fixed-output derivations and is the last item on the milestone's
critical path. cpp's answer for that row is
`/nix/store/c2h2f4cw9p8i8zcfy52fd1dd6g0yhnki-hello-2.12.3`.

`nixpkgs-frontier.sh` **exits non-zero on `differ > 0`**. A refusal is a gap
this backend admits to; a difference is two evaluators disagreeing about a
real expression, and only the second must not accumulate.

**Getting there cost one bug that looked exactly like the expected one, and
was not** (ENG-12593). With `unsafeGetAttrPos` answering `null` the row went
from REFUSED to DIFFER: `hello.outPath` raised `expected an integer or float
but found a set`. Since `hello.src` is a `fetchurl`, that reads as the
fixed-output work -- and a minimal fixed-output derivation refusing cleanly by
name is what separated the two. The real cause was `+`: it is cppnix's
`ExprConcatStrings`, whose string branch coerces a set through `__toString`
or `outPath`, and this backend coerced sets for interpolation (ENG-12513) but
not for `+`. Both operand positions were wrong, differently, and the left one
is the one a hasty fix misses -- in cppnix the *first* element decides the
branch, so `{ outPath = "/x"; } + "a"` is `"/xa"` and not arithmetic.

The lesson for sequencing: a DIFFER that appears where a REFUSED used to be
is not evidence about the refusal. It was worth one bisect and a four-case
golden taken from the cpp arm, one of which contradicted what the fix's author
expected (`__toString` beats `outPath` when both are present).

### How the wall was measured before it came down

Three blockers fell in one sitting -- `addErrorContext`, then `__functor`,
then `unsafeGetAttrPos` -- and each was invisible until the one before it was
gone. Rather than guess again, the wall was measured by climbing over it:
**a throwaway build in which `unsafeGetAttrPos` answers `null` for every
attribute**, which is a wrong answer and was never committed. What it found:

```
6  the package set                    AGREE    "set"
7  one package name                   AGREE    "hello-2.12.3"
9  stdenv                             AGREE    "stdenv-linux"
12 package set attr count             AGREE    27682
8  one package outPath                REFUSED  builtins.placeholder

RESULT nixpkgs-frontier rows=12 agree=11 differ=0 refused=1
```

**All 27,682 top-level attributes of nixpkgs enumerate and agree, `hello.name`
and `stdenv.name` agree, and the only remaining refusal was
`builtins.placeholder`** -- which is now implemented (pure, and pinned against
cppnix's bytes for three names, because it feeds `.drv` content and is
therefore tier 1). cppnix's answer for the milestone is
`/nix/store/c2h2f4cw9p8i8zcfy52fd1dd6g0yhnki-hello-2.12.3`.

So one tier-1 package attribute is close, and what stands between is a
correctness decision rather than a pile of builtins.

**Why `null` was not committed.** It is a wrong value where cppnix returns a
position, and four corpus files assert real positions -- they would go from
`unimplemented`, which this file counts as red, to `mismatch`, which is worse.
nixpkgs' own test calls the function "unspecified best-effort behavior"
(`lib/tests/modules/declaration-positions.nix:5`) and cppnix does return
`null` for attributes that have no position, so an allowlisted `null` is
arguable -- but it is a semantic divergence, and CLAUDE.md's bar says a person
decides those, not an agent in a hurry.

**The honest fix is attribute positions**, a bounded slice of ENG-12137: the
compiler already sees every attrset literal's span, so what is missing is a
per-module position table and a way for `Value::Attrs` to reach it. That also
retires `eval-okay-curpos`, the last rung A mismatch, and the
`addErrorContext` allowlist entry above. It is the next thing on this rung and
it is bigger than a day.

Behind it, in the order the probe found them and with `placeholder` already
gone: `builtins.path`, `filterSource` and `toFile` are the store-write family
(one mechanism, ENG-12491), and `hello.outPath` needs fixed-output
derivations in `drvstrict`, which today refuses `outputHash`/`outputHashAlgo`/
`outputHashMode` by name because a fixed-output path is `makeFixedOutputPath`
rather than the input-addressed arithmetic rung C step 1 measured.

The blocker before search paths was `builtins.nixVersion`, a constant that
appears nowhere in the lang corpus; `currentSystem`, `addErrorContext` and
`__functor` have each repeated the lesson since. Sequence by rung and by
probe, not by corpus file count.

## Remaining corpus mechanisms, by cost

Compile (6 files): `__overrides` in a rec set (3),
path interpolation (the `./${x}` form, which is a path literal built at
runtime -- not the `"${./x}"` string coercion, which ENG-12447 implemented),
`~/` home paths, `__curPos`.

Underscore digit separators left this list in ENG-13119. They did need an rnix
patch, as predicted, and that patch is now carried on
`indexable-inc/rnix-parser` at `ix-patched-0.12`, pinned by rev from
`rust/nix-eval-rs/Cargo.toml`. The branch is the place any further lexer
divergence goes -- lone-CR line endings (`eval-fail-eol-2` in
eval-allowlist.toml) is the next one, and it is now a patch on an existing
fork rather than a fork to create.

Evaluate (18 files): `unsafeGetAttrPos` (4, blocked on source positions in the
IR, ENG-12137), `trace` (2), `toXML` (2), then one file each for `scopedImport`,
`toPath`, `path`, `parseFlakeRef`, `parseDrvName`, `hashFile`, `fromTOML`
timestamps, `flakeRefToString`, `convertHash`, plus `__functor`, which is a
mismatch rather than unimplemented.

`trace`, `parseDrvName`, `hashFile`, `convertHash`, `toPath` and `toXML` are
small and worth roughly ten files between them, but none of them unblocks a
rung. Prefer rung C and search paths when the two compete.
