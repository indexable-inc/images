# `nix eval <flake>#attr` on the Rust backend: what is missing, and in what order

Written 2026-08-06 while landing the fetcher family (`builtins.fetchurl`,
`fetchTarball`, `fetchTree`, `fetchGit`) in `rust/nix-eval-rs`. Those four are
the bricks a flake entry stands on, and with them in place the question "what
else does `getFlake` need?" has a concrete answer rather than a guess. This
records it so the next person starts from the blockers rather than rediscovering
them.

Short version: **the flake entry is not blocked on `getFlake`.** It is blocked
on two things underneath it, one of which changes what the backend refuses
across the board. `getFlake` itself is a small amount of routing on top.

## Pure eval no longer refuses the question channel (done, 2026-08-06)

This is the blocker. Measured on dev-compute-3 at `694a3faed`, one binary, two
`eval-backend` settings:

```
$ nix-instantiate --eval --strict --pure-eval \
    -E 'builtins.fetchurl { url = "file:///nope"; sha256 = "sha256-Flke…VO4="; name = "hello-1.0.tar.gz"; }'

cpp:  "/nix/store/vyngj6x7baydvg0pqxazlihyx2pdwmc4-hello-1.0.tar.gz"
rust: error: rust-eval unimplemented: fetching 'file:///nope' with builtins.fetchurl
      under restrict-eval or pure-eval (this evaluator reads the filesystem
      outside cppnix's access control, so it cannot honour either setting)
```

cppnix serves that: the fetch is pinned, the store already holds the path, and
`ensurePath` answers without touching the network, so purity is not violated.
The Rust backend refuses it, and refuses every `import`, `readFile`,
`pathExists` and store question alongside — `answer_path` in `eval.rs` gates
the whole question channel on one flag, and the bridge sets that flag off for
`restrictEval || pureEval` (`rust-eval-session.cc:536`).

`nix eval nixpkgs#lib.version` runs under pure eval. So `fetchTree` and
`getFlake` can be implemented perfectly and no real flake will evaluate.

**What it cost.** The two settings stopped being one flag. `eval::Settings`
now carries `pure_eval` and `restrict_eval` separately, both destructured in
`fingerprint` (a new evaluator setting cannot compile without being placed
there — ENG-12541, deliberately), and `rust/nix-eval-rs/src/purity.rs` holds
one table row per `NeedPath` with the cppnix line each was read off.

The line the table draws is **not** the one guessed above, and the difference
matters. It is not "restrict-eval refuses wholesale, pure-eval decides per
question". It is **who answers**:

- A question the *embedder* answers is served under both settings, because the
  embedder is cppnix and applies cppnix's own access control on the way.
  `rustCopyToStore` and `rustStoreFiltered` go through
  `host.state.rootPath(...)`, i.e. `rootFS`; `rustFetch` and `rustFetchTree`
  call `state.checkURI`, and the tree fetch also runs cppnix's own
  `input.isLocked` check; `rustFindFile` and `rustNixPath` read
  `host.state.findFile` and `getLookupPath()`, which cppnix already built
  under these settings. Every one of those returns cppnix's own error text when
  the setting forbids it.
- A question *this crate's own `Host`* answers is refused, under either
  setting, because that `Host` is a plain `std::fs` reader that consults no
  allow list. That was `import`, `readFile`, `pathExists`, `readDir` and the
  path-kind query — five of the sixteen. **Closed 2026-08-06 by ENG-12792**;
  see the section below.
- Two questions the crate answers *without* the world: `getEnv` is `""` under
  either setting, which is cppnix's own rule (`primops.cc:1261`), and an
  unpinned `fetchurl`/`fetchTarball` under pure eval raises cppnix's
  `"in pure evaluation mode, '%s' requires a 'sha256' argument"`
  (`fetchTree.cc:537`) as an ordinary evaluation error rather than a refusal.

**One claim in the paragraph this replaces was wrong**, and it is worth
recording because it reads plausible. "A path outside the store refuses; a
store path is allowed" is not what cppnix does. Under pure eval the allow list
that `AllowListSourceAccessor` wraps `rootFS` in starts **empty** and grows only
through `allowPath` as the evaluator itself introduces store paths, so a store
path written literally into an expression is refused like any other. Measured
on nix 2.34.7+ix.h24085346:

```
$ nix-instantiate --eval --strict --pure-eval \
    -E 'builtins.readDir /nix/store/c2h2...-hello-2.12.3'
error: access to absolute path '/nix/store/c2h2...-hello-2.12.3' is forbidden
       in pure evaluation mode (use '--impure' to override)
```

`pathExists` on the same path answers `false` rather than failing, because
`prim_pathExists` catches `RestrictedPathError` (`primops.cc:2097`). It answers
the *truth* for an allowed path, though, so a backend that returned a blanket
`false` would be handing back a wrong value rather than a refusal — which is
why `Exists` is in the refusing five.

**Measured, before and after, two binaries built from the same tree**
(`751df4efd`, base and base plus this change, both `-Dnix:rust-eval=enabled`,
`nix-instantiate --eval --strict --pure-eval`, `eval-backend = rust`):

| expression | before | after |
| --- | --- | --- |
| `builtins.getEnv "HOME"` | refusal | `""` |
| `builtins.nixPath` | refusal | `[ ]` |
| `builtins.toFile "x" "hi"` | refusal | `"/nix/store/pmif2b...-x"` |
| `builtins.fetchurl { url = "file:///tmp/x"; sha256 = ...; }` | refusal | `"/nix/store/g89ais...-eng12541-hello.txt"`, byte-identical to the cpp arm |
| `builtins.fetchurl "https://…"` (no pin) | refusal | cppnix's own `requires a 'sha256' argument` |
| `builtins.readFile /etc/hostname` | refusal naming "restrict-eval or pure-eval" | refusal naming `pure-eval` |

The last row is the point of the split as much as the others: the old wording
named a setting the operator had not set.

## The five plain reads go through `rootFS` too (done, 2026-08-06)

ENG-12792, the remainder of ENG-12480. `import`, `readFile`, `pathExists`,
`readDir` and `readFileType` now reach the world through
`host.state.rootPath(...)` like every other served question, so the last
`Refuse` row is gone whenever an embedder is attached. The five hooks travel
as one set in the session's `IxeHostVtable` and a partial set is refused at
`ixe_session_new` — four of five would mean one question walking around the
allow list while the table said otherwise — and each hook is a transcription
of the cppnix primop that asks the same thing.

The standalone configuration still refuses. The probe, the differential
harness the cache-semantics gate builds and every unit test have no embedder,
and there `RealFs` still reads with `std::fs`. So `purity::verdict` takes a
`PathReads` argument beside the settings, and `eval::Settings` carries it into
the memo key: without that, a witness recorded standalone under `pure-eval`
(empty read set, result `unimplemented`) addresses the same row as the `nix`
binary's run and would be served to it.

**Measured, before and after, two binaries built from the same tree**
(`98d0536604` and `a36663b43a`, both `-Dnix:rust-eval=enabled`,
`nix-instantiate --eval --strict --pure-eval`, `eval-backend = rust`, each arm
probing `-E 1` first):

| expression | before | after |
| --- | --- | --- |
| `readFile` of a pinned `fetchurl` path | refusal | the contents, as cpp |
| `import` of a pinned `fetchurl` path | refusal | the attrset, as cpp |
| `pathExists` of it | refusal | `true` |
| `readFileType` of it | refusal | `"regular"` |
| `readDir` of a pinned `fetchTarball` tree | refusal | 2 entries, as cpp |
| `import` of that tree, i.e. its `default.nix` | refusal | `"hi from a fetched tree"` |
| a nested `import ./sub/x.nix` inside it | refusal | `42` |
| `readFile /etc/hostname` | this crate's refusal | cppnix's own `access to absolute path '/etc/hostname' is forbidden in pure evaluation mode` |
| `pathExists /etc/hostname` | refusal | `false`, as cpp |

The last two rows are the half worth reading twice: the change hands the
decision to the accessor that already had it rather than opening a hole, and
the refusal is now cppnix's own words rather than this backend's.

**These ran on an unclaimed node, and the table inherits that.** They ran on
dev-compute-5 while it was carrying another engineer's live workload -- 22
processes including a postgres four and a half days old -- and the claim tool
(`nix run .#ix-dev-claim`, ENG-9965) refuses that node for exactly that
reason. The node was never claimed through it. Nothing here detected
interference, and could not have: each row is the cpp arm of the same binary
as its own control, which catches a wrong answer but not a slow or perturbed
one. "No interference was detected" is a weaker statement than "the box was
exclusively mine", and only the second is what a measurement should be able to
make. Anyone re-running this should claim a node through the tool first; a
file written in a home directory is not a claim (ENG-12823).

`rust-nix-eval-gate.sh` section 8b holds it. It compares the two arms' refusals
against each other rather than against a string written here, refuses a case
where the cpp arm does not refuse at all, and has a served arm — six reads out
of a pinned `fetchTarball` path — that an all-refusing backend cannot pass.

Two deliberate gaps, both noted at the call site. `prim_readFile`'s reference
scan is not transcribed, so a string read out of the store does not carry the
references found in its bytes; that belongs with ENG-12465. And
`prim_readDir`'s lazy `readFileType` thunk is resolved eagerly, because the
boundary has no lazy field.

## The command layer refuses a flake installable before the evaluator exists

`nix eval <flake>#attr` never reaches the VM:

```
$ nix eval --raw nixpkgs#lib.version        # eval-backend = rust
error: rust-eval unimplemented: flake and store-path installables
       (this backend evaluates '--expr' and '--file' sources)
```

That refusal is in `src/nix/eval.cc`, in `run()`, before an `EvalState` is
built — and it is there for a good reason the comment states: `parseInstallables`
evaluates the source with the C++ evaluator on its way to building an
`InstallableAttrPath`, so routing after it would have already run the wrong
backend.

So the flake entry is a **command-layer** change as much as an evaluator one.
The shape that fits the existing code: `runWithRustBackend` learns a third
source kind beside `--expr` and `--file`, which resolves the flakeref through
`lockFlake` (C++, as it must be), gets a `LockedFlake`, and hands the evaluator
the `call-flake.nix` source plus its three arguments. `getFlake` as a *builtin*
is then the same routing reached from inside an expression rather than from the
command line, and both should go through one seam.

## `emitTreeAttrs` does not return plain attributes in this fork

This one is easy to miss and loses instrumentation silently.

`allocRecordedTreeAttr` (`src/libexpr/primops/fetchTree.cc:41`) is ix-local.
When `state.readSetTracker` is set, every tree metadata attribute — `narHash`,
`rev`, `shortRev`, `revCount`, `lastModified`, `lastModifiedDate` — is not a
value but a **thunk holding a one-attribute primop**, which records the read
when something forces it. That is what makes "how deep does the revision reach"
answerable: a flake entry that never looks at `rev` does not acquire it as an
input.

A `fetchTree` host question that answers with a flat attribute set loses all of
it, and loses it quietly — the values are right, the store path is right, and
the provenance graph is just thinner than it was.

The tree fetchers as landed **refuse rather than lose it**: `rustFetchTree`
returns status 2 ("this embedder will not serve this") when the tracker is on,
which surfaces as a named refusal via `StoreError::Unsupported`. Serving it
would have been wrong twice over — the JSON serialiser forces every thunk, so
it would record reads the program never made *and* hand the evaluator plain
values that can never record the reads it does make.

Closing it properly needs the VM to grow the same shape: a slot that, when
forced, emits a `NeedPath` naming the tree and the attribute, then yields the
value. The VM already has lazy slots and resumable machines, so this is not a
new mechanism — but it is a new host question and a new `Host` method, and the
recorded output has to reach the same `ReadSetTracker` the C++ side writes to.

## What `getFlake` itself needs, once those are done

Less than the above, which is the point of writing this down.

- **`call-flake.nix` is ordinary Nix.** 105 lines using `fromJSON`, `mapAttrs`,
  `substring`, `removeAttrs`, `head`, `tail`, `import`, recursion through a
  `let`-bound `allNodes`, and one `assert`. The VM has all of it. It needs no
  new evaluator feature; it needs its three arguments.
- **Its three arguments are embedder data.** The lock file as a JSON string,
  an `overrides` attrset built from `lockedFlake.nodePaths` (each entry an
  `emitTreeAttrs` set plus a `dir`), and `fetchTreeFinal` — which is
  `prim_fetchFinalTree`, i.e. `fetchTree` with `isFinal` set. The first two are
  values the bridge constructs and hands over; the third is a fourth variant of
  the tree question, differing only in the `__final` attribute.
- **Locking and resolution stay in C++.** `lockFlake` walks the input graph,
  hits the registry, writes `flake.lock`. It is IO and policy, cppnix owns it,
  and nothing about it belongs in the VM. This part of the brief's original
  guess was right.

So the ordering is:

1. ~~`pure-eval` as a real evaluator setting, distinct from `restrict-eval`.~~
   Done, 2026-08-06, ENG-12541 part 2. See the section above for what the
   table actually says and for the one claim in the original plan that
   measurement contradicted.
1a. ~~The five plain reads through `rootFS`, so files can be read and imported
   out of a fetched store path under pure eval.~~ Done, 2026-08-06,
   ENG-12792. This was the evaluator-side half of the flake entry; what
   remains below is the command layer and the tree fetchers.
2. Recorded tree attributes in the VM, or a written decision to accept the loss
   under `eval-backend = rust` while the tracker is on. Today it is a refusal,
   which is the honest interim.
3. `fetchFinalTree` — a flag on the existing tree question.
4. ~~The command-layer seam in `src/nix/eval.cc`, and `getFlake` on the same
   seam.~~ Both done: the command layer in PR #142, `getFlake` in ENG-12995.
   See the section below for what "the same seam" turned out to mean and how
   it is now checked rather than asserted.

## Flakes with inputs run, and pre-locking is what decides the code path

Added 2026-08-06 with `maintainers/ix/flake-inputs-parity.sh`, which is the
gate that closes the "largest untested area" the list below used to name.
Seven fixtures -- one absolute `path:` input, one relative `path:` input, a
`tarball+file://`, a `git+file://`, a `follows` redirection, an input of an
input, and a `flake = false` input -- times five attributes each. 35 of 35
rows byte-identical between `eval-backend = cpp` and `eval-backend = rust`,
including seven `drvPath`/`outPath` pairs whose `.drv` was opened and hashed
rather than printed. Measured on this Mac, aarch64-darwin, one binary, two
settings. Everything is local: `path:`, `file://` and a git repository made by
the gate, so it runs offline.

**The finding worth carrying out of it: whether a flake is already locked
changes which half of `call-flake.nix` runs, and the two halves are
indistinguishable in every value they produce.**

`computeLocks` fills `nodePaths` only for nodes it actually fetches. On the
run that CREATES a lock file every node is fetched, so every node lands in
`nodePaths`, `flakeOverridesJSON` hands `call-flake.nix` an override for every
one of them, `hasOverride` is true everywhere, and `fetchTreeFinal` is
unreachable. On a run against an up-to-date lock the `!mustRefetch` branch
keeps the child lazily and never adds it, so the override is absent and
`call-flake.nix` calls `fetchTreeFinal`. Measured, same two-node flake, same
binary, the only difference being whether `flake.lock` already existed:

```
first  run (creates the lock):  rust-eval: flake overrides cover 2 of 2 lock node(s)
second run (lock up to date):   rust-eval: flake overrides cover 1 of 2 lock node(s)
```

That line is new, emitted at `debug` from `flakeOverridesJSON`, and it exists
because nothing else distinguishes the two runs: an overridden node and a
fetched one yield the same store path, the same `narHash` and the same drv, so
a gate comparing values cannot tell which one it measured. The gate reads it
and refuses a run where no node reached the fetcher.

Two consequences.

- A relative-path node's `sourceInfo` carries **only** `outPath`. Every other
  fixture answers `["lastModified", "lastModifiedDate", "narHash", "outPath"]`;
  a relative one answers `["outPath"]`, on both arms, because there is no
  independent tree to hash. Worth knowing before writing an assertion about a
  flake input's metadata: asking a relative node for `narHash` fails with
  `attribute 'narHash' missing`, and it fails on cppnix too.
- A measurement taken against a freshly-locked flake has not exercised the
  tree fetcher, whatever it says. The earlier flake evidence (`drv-parity.sh`'s
  no-input fixture) is in that position by construction: its only node is the
  root, which always carries an override.
- The relative-path branch and the `fetchTreeFinal` branch are mutually
  exclusive, and not by accident. A kept flake whose lock subtree holds a
  relative-path input forces `mustRefetch` (the NixOS/nix#14762 guard), which
  re-fetches and so re-adds the node to `nodePaths`. `relpath` is the one
  fixture of the seven that reads `all-overridden`, and the gate asserts that
  per fixture rather than in aggregate, so the day it changes is loud.

**What the seven fixtures do not cover, stated plainly.** `github:` proper.
It cannot be fetched without a network and a test needing a network is a test
that flakes, so the tarball fixture stands in for it. That substitution buys
the evaluator-side surface -- a locked non-path node fetched through
`fetchTreeFinal` whose `sourceInfo` crosses the JSON boundary -- and buys
nothing about `GitArchiveInputScheme`'s own resolution: the API call, the
rev-to-tarball mapping, the `lastModified` read out of a header. None of those
is evaluator code, but none of them is covered either.

## `builtins.getFlake` runs on the VM, on one seam with the command line

ENG-12995, 2026-08-06. `prim_getFlake` is two halves with a clean line between
them, and the line is the charter's: everything up to `callFlake` is locking --
parsing the reference, the pure-eval rule that refuses an unlocked one, the
registry, the input-graph walk, the fetches -- which is IO and policy the
embedder owns, and `callFlake` is an ordinary Nix application the VM performs.

So `getFlake` is one new host question, `NeedPath::Flake`, answered by
`rustLockFlake` with three documents: `call-flake.nix` itself, the lock file,
and the overrides. `rustLockFlake` and `rustEvaluandOf` call the same
`flake::callFlakeSource()` and the same `flakeOverridesJSON`, so there is one
program and one overrides document with two ways in. The `call-flake.nix`
source is sent over the boundary rather than embedded in the crate for the same
reason: a copy on each side is two copies of the 105-line program that decides
which tree every flake input resolves to.

**"One seam" is checked, not asserted.** `flake-inputs-parity.sh` now asks all
eight fixtures a second time through `builtins.getFlake` and scores each row
twice: rust-getFlake against cpp-getFlake, and rust-getFlake against the rust
*command line* for the same flake. 40 of 40 both ways. The second comparison is
the one that matters, because a cross-arm comparison alone would pass if both
entry points were wrong together -- if `getFlake` grew a second, subtly
different overrides document, both arms would agree with themselves and the
store paths would move in step. The selftest's `oracle-blind` case is that
scenario made real: it points both getFlake arms at a different fixture, the
cross-arm rows stay green, and only the oracle fires.

Measured on this Mac, one fixture with one path input, all four spellings:

| | drvPath | outPath |
| --- | --- | --- |
| cpp, command line | `…acz4kvjn…-gf-19184.drv` | `…40g05ncz…-gf-19184` |
| cpp, `getFlake` | identical | identical |
| rust, command line | identical | identical |
| rust, `getFlake` | identical | identical |

Provenance on the `getFlake` run: `{cpp: 0, cppFlakeLock: 2, rust: 1}`, so the
C++ evaluation is confined to locking and counted apart, exactly as on the
command line.

**Two things it does not change.** The read-set tracker still refuses, now
under `token=unimplemented-builtin` rather than the installable path's
`command-unsupported` -- a different token on purpose, because here the command
runs fine and a *builtin* is what cannot be served. And membership is
ungated: `getFlake` is not in `CPP_PRIMOP_NAMES` (the generator scans the
primops.cc family, and this one is registered from libflake), the sources
declare no gate, and two generator tests refuse a hand-added one. The behaviour
agrees: with flakes disabled, `builtins ? getFlake` is `true` on both arms and
calling it errors with cppnix's experimental-feature message on both.

**Three more shapes were probed and agree; two more refuse by name.** Run once
each on this Mac, both arms of one binary, not gated (except `?dir=`, which
became the eighth fixture): `self.outPath` inside a flake with an input,
`?dir=sub` selecting a subdirectory of a fetched tree, and `--override-input`
pointing an input at a different flake all produced identical bytes.
`builtins.getFlake` refuses (`unimplemented-builtin`, ENG-12995) where the
same flake on the command line is served; `nix flake show` and `nix flake
metadata` refuse by name because those commands have no Rust path.

**The read-set tracker refusal is no longer merely written.** The list below
used to say it "has not been provoked". It has now, on a flake with one path
input, `read-set-trace-file` set, both arms of one binary:

```
rust: error: rust-eval unimplemented: a flake installable while the read-set
      tracker is on (…)      token=command-unsupported
cpp:  r+dep-one
```

So it is a named refusal and not a wrong answer, which is what the parity bar
asks of a gap. It is still a gap: with the tracker on, no flake evaluates on
this backend.

## What has NOT been looked at

Named, because the list above reads like a plan and a plan with unexamined
corners is worse than one that says where they are.

- **cppnix's eval cache.** `lockedFlake.getFingerprint` keys
  `state.evalCaches`, and the Rust backend does not use it. That is not an
  omission to close: `AttrCursor` walks cppnix `Value`s and there are none on
  this path -- the bridge walks the VM's handle table -- so there is nothing
  for that cache to hold. The two caches therefore never cover one evaluation;
  under `eval-backend = rust` a flake's outputs are memoised by `ix-kernel`
  and by nothing else.

  The `ix-kernel` half was the open soundness question and is settled
  (ENG-12915). The memo key carries the applied arguments -- the lock file
  text and the overrides document, byte for byte, behind a kind tag -- so two
  flakes are two rows. What that key is *not* carrying, said plainly, is
  anything about the locking step itself: the registry lookups, the input-graph
  walk and the fetches `lockFlake` performs happen in C++ before the question
  is asked, on every invocation, and are never memoised. A warm run re-locks
  and then skips evaluating `outputs` for that lock. That is the whole claim,
  and it is why the pre-lock reads do not need to be in the recorded read set:
  they are not being skipped.
- **`nix develop`, `nix flake show`, `nix flake metadata`.** `nix eval` and
  `nix build` have a Rust path, and `builtins.getFlake` reaches the same
  machinery from inside an expression (ENG-12995); the others parse
  installables their own way.
  Measured 2026-08-06 on a flake with one path input: `nix flake show` and
  `nix flake metadata` refuse by name -- "`eval-backend = rust` selects an
  evaluator this command has no path to, and continuing would have silently
  used the C++ evaluator" -- which is the right shape for a gap, and cpp
  serves both.
- ~~**Relative path inputs.**~~ Run, 2026-08-06, along with the rest of the
  multi-node machinery. See the section above: the `isRelative` branch, the
  `follows` list in `resolveInput`, `getInputByPath`'s recursion, the
  `flake = false` branch and `fetchTreeFinal` all have a fixture now, and the
  surprise was that the last two of those are mutually exclusive with the
  first.
- **`--override-input` and `--impure` flake evaluation**, which change what
  `overrides` contains. `--override-input` was run once, 2026-08-06, on a
  one-input flake pointed at a second flake, and the two arms agreed; that is
  a probe and not a gate, and `--impure` is still untouched. The reason to
  keep this on the list is that an override changes the `nodePaths` set, which
  is the same thing pre-locking changes, and the section above is about how
  invisible that is.
- **The read-set tracker.** A flake installable refuses by name
  (`command-unsupported`) while `readSetTracker` is on, for the reason
  `rustFetchTree` does: `emitTreeAttrs` answers with a per-attribute recording
  thunk under the tracker, and the overrides document forces every one of
  them. Provoked 2026-08-06 and it fires with that token; the gap it names is
  still open, so no flake evaluates on this backend with the tracker on.
