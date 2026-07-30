# Incremental evaluation for the ix Nix fork

Status: design proposal, not implemented. Written 2026-07-29 against
`indexable-inc/nix` at `cd6043f41` (`ix megamerge: 47 patches on 2c6d06e9387c`).

## Recommendation, in one screen

Build a **persistent evaluator process** that keeps `EvalState` and its Boehm
heap alive across invocations, invalidates only the thunks whose recorded input
set changed, and re-forces those. Do **not** attempt to serialise a
partially-evaluated heap; do not attempt per-thunk read sets.

Three findings drive that recommendation, and each is a fact about this
codebase rather than a preference:

1. **A `Value` is exactly 16 bytes with no spare bits.** The bit-packed
   `ValueStorage` specialisation in `src/libexpr/include/nix/expr/value.hh:574`
   uses all three low bits of `payload[0]` for the primary discriminator and,
   in the pair-of-pointers case, all three low bits of `payload[1]` for the
   secondary. There is nowhere to hang a read set. Adding one word per `Value`
   costs 50% of the value heap. At 33.8M thunks in a single host evaluation,
   8 bytes per thunk is 271 MB and 16 bytes is 542 MB, added to an evaluation
   that already allocates about 3.5 GB. Per-thunk tracking is dead on arrival.

2. **Forcing a thunk destroys the thunk in place.** `EvalState::forceValue`
   (`src/libexpr/include/nix/expr/eval-inline.hh:96`) reads `env` and `expr`
   out of the thunk, writes a blackhole over the same `Value`, and then lets
   `expr->eval(*this, *env, v)` overwrite it with the result. After forcing,
   the `(Env *, Expr *)` pair that produced the value is gone. Salsa can
   re-execute a query because it kept the key; Nix throws the key away. Any
   incremental design has to buy that back, and buying it back is most of the
   memory cost of the whole feature.

3. **Whole-tree source granularity would make thunk-level incrementality
   worthless, and the fork already carries the fix.** The 31 changed closure
   paths root at `source`, the flake's own store path for the entire tree.
   `Flake::getFingerprint` (`src/libflake/flake.cc:1127`) hashes the whole
   input plus the lock file into one value, which is why the eval cache is
   130x or nothing. If a thunk's recorded input is "the tree", one edited byte
   dirties every thunk that touched it and incremental evaluation buys zero.
   The fork has lazy trees behind an off-by-default setting
   (`lazy-trees`, `src/libexpr/include/nix/expr/eval-settings.hh:299`, plus
   `ce4e55576 libexpr: snapshot mutable source trees at mount time`). **Lazy
   trees is a prerequisite for this project, not an adjacent optimisation.**

The honest scope: this makes the *evaluation* loop fast. It does not make
deploys fast. A one-line comment still changes `source`, still changes the
`drvPath`, and still rebuilds the 31 paths that embed the tree. The prize is
turning the twelve-node eval from 247s into something in the low seconds after
a small edit, which turns evaluation into a linter you can run in a commit
hook instead of a thing you discover during a 74-minute deploy.

**If you only read this far:** fund phase 1, which is a measurement and not a
feature. It is a few weeks of work and it answers the one question that decides
whether the rest is worth anything: after a realistic edit, what fraction of
the 33.8M thunks is genuinely invalidated? Every estimate below is conditional
on that number, and I could not obtain it without building the instrumentation.

## The measured anchor

From `hil-compute-1` in ix, evaluating `config.system.build.toplevel.drvPath`
for one host:

| run | cpuTime | thunks | drvPath |
| --- | --- | --- | --- |
| baseline | 24.85s | 33,846,982 | `2cys5c42...` |
| baseline repeated | 22.17s | 33,277,654 | `2cys5c42...` identical |
| one comment added | 22.36s | 33,846,983 | `89p4bdcm...` different |

Closure comparison of the two results: 20,461 paths each, 31 changed, 20,430
byte-identical. 99.85% of the output is unchanged and 100% of the work is
re-paid.

Two details in that table matter more than the headline:

- The thunk count differs by **exactly one** between baseline and
  comment-added. The shape of the computation is essentially identical, which
  is the precondition for any matching scheme working at all.
- The two baseline runs differ by 569,328 thunks (1.7%) despite producing an
  identical `drvPath`. Evaluation is not thunk-for-thunk deterministic across
  runs. Whatever the cause (this is unverified; likely candidates are
  `nrThunks` being a process-global `static Counter` at
  `src/libexpr/eval.cc:939` combined with differing store or fetcher cache
  state), a design that assumes an exactly reproducible thunk graph is
  assuming something the measurements contradict.

Separately, the flake eval cache gives 1.74s to 1.94s for twelve nodes with an
unchanged tree against 247s to 259s when anything changes. A 130x cliff with
nothing between, because the fingerprint is the whole key. That cliff is the
shape of the problem: Nix today has perfect caching at a granularity nobody's
workflow fits.

### What the 0.15% does and does not tell you

It says the *result* barely moves. It does **not** bound the fraction of
invalidated thunks, and the temptation to read it as "99.85% of evaluation is
reusable" is the single most likely way to oversell this project.

The reason is fan-out from a single input. `source` is one input, read by
everything that interpolates a path from the flake. If `${self}` or a
`self`-derived path is threaded widely (into `imports`, into `pkgs` overlays,
into `_module.args`), then a one-byte change to the tree dirties an
unboundedly large share of the graph even though only 31 output paths move.
The 31 paths are the paths that *embed* the tree; the thunks that *read* it
may be far more numerous.

This is why phase 1 is a measurement and not a feature.

## Why the existing caches cannot be stretched to cover this

Worth stating because "just improve the eval cache" is the first thing anyone
proposes, and it has been tried three times upstream (see the section on
stalled designs).

The flake eval cache is a SQLite table with the schema at
`src/libexpr/eval-cache.cc:35`:

```sql
create table if not exists Attributes (
    parent integer not null, name text, type integer not null,
    value text, context text, primary key (parent, name)
);
```

That is a memo of attribute-path traversals to scalars: a tree of
`(parent, name) -> value` rows for strings, ints, bools and placeholders. It
caches what the CLI asked for, keyed on a whole-tree fingerprint. roberth
documented that expectation explicitly in NixOS/nix#11322 ("doc: Manage
expectations for eval-cache", merged 2024-08-19), quoting Arian van Putten:
"Flakes doesn't have eval caching. It has command line argument caching."

Three properties make it unstretchable:

- **It stores values, not computations.** There is no `Env`, no `Expr`, no
  dependency edge. It can answer "what was `x.y.z` last time" and nothing else.
- **It only holds serialisable leaves.** A function, a partially-applied
  `mkDerivation`, a module fixpoint: none of these have a representation here.
  The interesting 22 seconds is spent producing exactly those.
- **Its key is a whole-tree hash.** By construction it cannot express "this
  value depended on these four files".

Extending it to hold computations is heap serialisation with extra steps. See
the next section for why that is the wrong problem.

## Decision 1: persistence. The persistent process dominates.

**Effect: choosing the persistent process removes serialisation from the
project entirely and replaces it with invalidation tracking, which is strictly
smaller and independently useful.**

Three candidate answers to "where does partial evaluation live between runs".

### Option A: serialise the heap (rejected)

Write out enough of the value graph that a later process can resume it. This
is heap snapshotting, and in this codebase specifically it means:

- Serialising `Env` chains. `Env` is `{ Env * up; Value * values[0]; }`
  (`src/libexpr/include/nix/expr/eval.hh:174`), a flexible array member with
  no recorded length; the size lives only in the `StaticEnv` that the parser
  built. Reconstructing an `Env` requires the `Expr` tree that shaped it.
- Serialising `Expr *` pointers, which point into per-file AST arenas
  (`Exprs`, `src/libexpr/include/nix/expr/nixexpr.hh:828`) with interned
  `Symbol`s and `PosIdx` offsets into a global `PosTable`. All three are
  process-local integer or pointer identities.
- Handling `PrimOp *` and `ExternalValueBase *`, which point at C++ code and
  at objects owned by libstore.
- Doing all of this under Boehm GC, which is conservative, non-moving, and has
  no object graph walk you can borrow. Values are batch-allocated out of
  `GC_malloc_many` (`src/libexpr/include/nix/expr/eval-inline.hh:38`), so
  there is not even a coherent per-object header to key on.

The killer is not difficulty, it is that the work is unbounded and the payoff
is bounded. You are writing a moving-GC-shaped serialiser for a
conservative-GC heap in order to save a process start. Reject.

### Option B: persistent evaluator process (recommended)

A long-lived daemon owns `EvalState` and the heap. Clients ask it for
`nixosConfigurations.<host>.config.system.build.toplevel.drvPath` and it
answers. Between requests it keeps everything: parsed ASTs, symbol table,
position table, every forced `Value`, every `Env`.

What this buys, in order of importance:

- **Nothing is serialised.** Thunk identity is pointer identity. The hardest
  problem in the project simply does not arise (with one boundary case, see
  Decision 3).
- **The problem becomes invalidation.** You need to know which files each
  cached value read, and you need to be able to put a value back into its
  pre-forced thunk state. Both are tractable and both are separately useful:
  the read-set data alone answers "why did this rebuild", which is a question
  the `nix-debugger` skill exists to answer today by other means.
- **It composes with what is already built.** Parallel eval (lever B) and GC
  heap sizing (lever 1) both help a daemon more than they help a one-shot
  process, because a daemon pays the heap-growth cost once.

There is a working prototype of the "keep the heap alive" half already in
tree: `nix repl` holds `EvalState` across commands, and `:r` re-runs
`reloadFilesAndFlakes` (`src/libcmd/repl.cc:761`), which calls `initEnv` and
reloads from scratch. That is the naive version of exactly this design, with
invalidation replaced by "throw everything away". Phase 2 can be built by
making `:r` incremental, which is a much smaller starting surface than a new
daemon.

Costs, stated plainly:

- **Memory.** One host evaluation allocates about 3.5 GB. Twelve hosts in one
  process is not 12x3.5 GB (nixpkgs is shared) but it is not 3.5 GB either,
  and this is unmeasured. Boehm does not return memory to the OS by default,
  so the daemon's RSS is a high-water mark, not a working set. On a compute
  node carrying VMs, a 20 GB evaluator is a real cost and needs a memory
  ceiling with a defined behaviour on hitting it (evict a host's roots and
  re-evaluate on demand).
- **Process lifetime.** Somebody has to own "when does this restart". A crash
  mid-request must not lose other hosts' work, which argues for the daemon
  being a supervisor plus per-host worker processes rather than one address
  space. That is a bigger design than it first looks and it is the part I
  would expect to be underestimated.
- **Store coherence.** The evaluator caches store queries. A GC run between
  requests can invalidate a cached `drvPath`'s existence, which is precisely
  the failure the flake eval cache already handles by re-checking that the
  `.drv` still exists (documented in the 2020 Tweag eval-cache post). A
  daemon has the same obligation over a longer window, and it must also
  notice `nix store gc` and IFD results appearing.
- **Evaluator upgrade.** The daemon's cached heap is only valid for the exact
  evaluator binary that produced it. Version the socket, refuse a mismatched
  client, and drain on upgrade. This is genuinely easy and it is the one cost
  item I am confident is small.
- **Purity becomes load-bearing.** A daemon that has cached
  `builtins.getEnv "USER"` for one client and serves another is wrong. Run
  the daemon under `pure-eval` and refuse impure requests, rather than trying
  to track impurity per value.

### Option B-prime: fork as the snapshot mechanism (evaluate this first)

**Effect: on Linux, `fork()` is a copy-on-write heap snapshot for free, so a
useful fraction of Option B's benefit may be available with no invalidation
tracking at all.**

The daemon evaluates up to a chosen point, then `fork()`s per request. The
child mutates its own COW pages and exits; the parent's snapshot is pristine.
This is the Android zygote model and it needs no read sets, no dirty
propagation, and no cross-run identity. It is perhaps two weeks of work rather
than two quarters.

Its value depends entirely on whether a useful *prefix* of the evaluation
exists, and Nix's laziness argues that it might not: the module system forces
`config`, which pulls packages on demand, so nixpkgs evaluation is interleaved
with configuration evaluation rather than preceding it. A snapshot point after
`import nixpkgs` and after the module list is resolved may capture only a
small share of the 22 seconds.

I did not measure this and it is measurable with the same instrumentation
phase 1 needs. **Phase 1 should report the fork-snapshot number alongside the
invalidation number**, because if the prefix is worth 60% of the evaluation
then the correct answer to this whole document is "build the zygote, stop
there" and several months are saved.

Caveats: `fork()` plus Boehm GC needs care (`GC_atfork_prepare` and friends);
this is aarch64-darwin-hostile, so it is a Linux-only optimisation and the
Mac keeps the slow path; and it does not help the twelve-node case any more
than the shared prefix is shared.

### Option C: on-disk value cache, done properly (defer)

Serialise only fully-forced, self-contained subgraphs (no `Env`, no free
variables, no functions) keyed by a content hash of their inputs. This is
`builtins.cachedImport` from the tweag/epcb draft, and it is the correct
shape for the one thing a daemon cannot do: survive a reboot, and be shared
between two machines or two CI runners.

Defer it to phase 3 and only build it if phase 2 shows that daemon warm-up
cost dominates in practice. Its cost is the same purity work epcb never
finished, and its benefit only appears in the cold-start case.

## Decision 2: granularity. Track boundaries, not thunks.

**Effect: recording anything per thunk costs hundreds of megabytes and buys
tracking you cannot use; recording read sets at a few thousand designated
boundaries costs single-digit megabytes and captures the reuse that matters.**

### The per-thunk cost, computed

One host evaluation allocates 33,846,982 thunks. Per-thunk overhead:

| what you store per thunk | bytes | total |
| --- | --- | --- |
| interned read-set id | 4 | 135 MB |
| one pointer (read set, or dirty link) | 8 | 271 MB |
| `(Env *, Expr *)` to permit re-forcing | 16 | 542 MB |
| the above plus a read-set id | 24 | 812 MB |

Against a 3.5 GB baseline these are 4%, 8%, 15% and 23% increases in
allocation, before counting the read-set contents themselves and before
counting the cost of the hash table that maps thunk to entry. And these cannot
live in the `Value`: a `Value` is 16 bytes, `alignas(16)`, with a payload of
exactly `std::array<uint64_t, 2>` and every alignment niche already spent
(`value.hh:574` through `value.hh:735`). Widening `Value` to 24 bytes changes
cache behaviour on the hottest structure in the evaluator, and Nickel hit the
same wall from the other side: they feature-gated their incremental work
specifically "because the current approach requires to increase the size of
thunks, which might penalize non-incremental runs a bit" (nickel-lang/nickel
PR #2484).

So a side table, keyed on `Value *`. Which means a hash lookup on a path that
currently runs about 33.8M times in 22 seconds, or roughly 1.5M
thunk-allocations per second. thufschmitt's #4511 measured 4% to 10% overhead
for adding a *word to `Attrs`* and a no-op lookup on attribute access, and
considered that unacceptable enough to abandon. A hash lookup per thunk
allocation will be worse.

**Conclusion: per-thunk anything is out.** This is the constraint the brief
predicted and it holds.

### What to track instead

Borrow three ideas that all point the same way.

- **Salsa's firewall pattern.** Not every function is a tracked query. You
  choose a small number of query boundaries and let the work between them be
  ordinary computation, then rely on early cutoff at the boundary to stop
  propagation. rust-analyzer's canonical example is that parsing shields
  everything above it from whitespace changes.
- **Nickel's "thunks of interest."** Their `semantic_hash.rs` states the
  criterion directly: a thunk worth caching should "be rather costly to
  compute (otherwise, it might be cheaper to recompute them from scratch) and
  have good chances of surviving successive changes (e.g focusing on top-level
  configuration fields rather than local variables)."
- **Adapton's demanded computation graph.** Only thunks that were actually
  demanded are nodes. Nix's laziness gives you this for free: a value nobody
  forced needs no entry.

For a NixOS evaluation the natural boundaries, in decreasing confidence:

1. **`import` results.** One entry per `(SourcePath, argument identity)`. This
   is the single highest-value boundary and it is where Nix's own three stalled
   designs all converged (`builtins.cachedImport`, the getFlake cache, the
   caching primop). Count: order of the number of `.nix` files evaluated,
   thousands to tens of thousands.
2. **Option values in the module fixpoint.** `config.<path>` and
   `options.<path>.value`. Count: order 10^4 per host. These are the boundaries
   an operator actually edits behind.
3. **Package derivations.** `pkgs.<name>.drvPath` and `.outPath`. Count: order
   10^4 for a NixOS closure.
4. **`callPackage` applications**, keyed on the function's file plus the
   argument set's identity.

Total order 10^5 tracked entries per host rather than 3.4x10^7 thunks: **two to
three orders of magnitude cheaper**, and at that size you can afford a real
read set (a sorted vector of interned input ids) per entry rather than a
compressed id.

Interning matters at this size too: distinct read sets are far fewer than
entries, because most option values in one module read the same handful of
files. Intern the read set, store a 4-byte id, and keep a
`read-set-id -> input ids` table. This is the standard trick and it is what
makes the memory arithmetic comfortable rather than tight.

### What "input" means, precisely

The tracked inputs are, exhaustively for `pure-eval`:

- **File contents**, keyed by `(SourceAccessor, CanonPath)`. This is where lazy
  trees earns its prerequisite status: without it, the accessor is one store
  path for the whole tree and every entry's read set is `{tree}`. With it, the
  read set of `config.networking.hostName` for one host can be four files.
- **Directory listings** (`readDir`, `pathExists`, `readFileType`). Distinct
  from contents: adding a file changes a listing without changing any content.
- **Flake input locks.** Already content-addressed in `flake.lock`, already the
  right granularity, and already hashed into the fingerprint at
  `flake.cc:1131`.
- **Source positions**, which are inputs because `builtins.unsafeGetAttrPos`
  observes them. See Decision 3; this one is not optional and it is not small.
- **The store**, for IFD and `builtins.storePath`. Keyed on the store path,
  which is content-addressed, so this is cheap and correct.
- **`builtins.currentSystem`, `currentTime`, `getEnv`, `nixPath`.** Under
  `pure-eval` most are unavailable; `currentSystem` is available and is a
  single global input. Refuse impure requests in the daemon rather than
  tracking these.

Recording a read of an input costs one push onto the currently-active entry's
read set. That requires a stack of active entries, which is the same shape as
Salsa's `ZalsaLocal` active query stack and the same shape as the existing
`EvalState` call-depth tracking. Cost is bounded by the number of input reads,
not by the number of thunks, and input reads are orders of magnitude rarer.

### The invalidation algorithm

This is "verifying traces" from Build Systems a la Carte (Mokhov, Mitchell and
Peyton Jones, ICFP 2018, section 4.2.2; JFP 2020 section 5.2), with a
suspending scheduler, which is the Shake quadrant of their table. Notably, the
same paper places Nix's *derivation layer* in the deep-constructive-traces row,
which "cannot support early cutoff." That is the paper's own diagnosis of the
gap this project fills: Nix has no early cutoff, at either layer.

Per request:

1. Diff the input set against the last revision. For files this is a stat plus
   hash, or an inotify watch, over the tracked files only.
2. For each changed input, walk the reverse index to the entries that read it
   and mark them dirty. This is Adapton's dirtying phase and it is `O(edges
   touched)`, not `O(graph)`.
3. On demand, when an entry is requested: if clean, return the cached value. If
   dirty, re-verify by checking each recorded input's current hash. If all
   match (a whitespace-only edit elsewhere in a file whose relevant bytes did
   not move), mark clean and return the cached value. **This is early cutoff
   and it is the entire source of the win.**
4. If an input genuinely changed, re-force. Re-forcing needs the original
   `(Env *, Expr *)`, which per finding 2 above was destroyed. So each tracked
   entry stores its `(Env *, Expr *)` alongside its value. At 10^5 entries that
   is 1.6 MB, which is why boundary tracking works and per-thunk tracking does
   not.
5. Keeping `Env *` alive keeps its whole environment chain alive, which pins
   heap that would otherwise be collected. **This is the memory cost nobody
   mentions**: the retained set of a persistent evaluator is larger than the
   live set of a one-shot evaluation, possibly much larger, and it is
   unmeasured. Phase 1 must report it.

Add **durability levels** from Salsa. Three classes: nixpkgs and flake inputs
(high, changes when `flake.lock` moves), the ix tree's library code (medium),
and the host configurations under active edit (low). Keep a version vector
rather than a scalar revision; incrementing a low-durability version leaves the
high-durability component untouched, so entries that transitively read only
nixpkgs are validated by one integer comparison instead of a graph walk.
rust-analyzer measured this as worth about 300ms per keystroke on the standard
library alone ("Durable Incrementality", 2023-07-24). For ix the split is
sharper than rust-analyzer's, because nixpkgs is pinned by a lock file and
genuinely never changes between two evaluations of an edited host config.

## Decision 3: identity across runs, and the edits this cannot handle

**Effect: in a persistent process the identity problem mostly dissolves, but
`builtins.unsafeGetAttrPos` in the NixOS module system reintroduces it in a
form that costs real reuse on any edit that shifts lines.**

### Why it mostly dissolves

The brief's concern is that source position shifts when a file is edited,
which is exactly the case that matters. In a persistent process:

- For an **unedited** file, nothing shifts. The `Exprs` arena is the same
  object, every `Expr *` is the same pointer, every `Symbol` and `PosIdx` is
  the same integer. Identity is pointer identity and it is free.
- For an **edited** file, everything shifts, and it does not matter, because
  every entry rooted in that file has to be recomputed anyway. That is the
  0.15% you are willing to pay for.

What remains is the boundary: an entry whose `Expr` lives in an unedited file
but whose `Env` chain reaches values produced by an edited file. Those must be
re-forced, and they can be, because the entry retained its `(Env *, Expr *)`.
The `Env` still points at the *old* values, so re-forcing requires rebuilding
the `Env` with the new ones, which means an entry's identity must include its
environment. Handle this by keying tracked entries on `(Expr *, argument
identity)` rather than on `Expr *` alone, exactly as Nickel's semantic hash
"combines the CUI of its core expression with the CUI of its dependencies."
For the `import` boundary the argument identity is the argument attrset's own
entry ids, which are already tracked. This is the recursive-hashing part of
the design and it is the part most likely to contain a soundness bug.

### The part that does not dissolve: positions are observed inputs

Verified from nixpkgs `lib/modules.nix` on master. `mergeModules'` computes,
for every option in every module:

```nix
mapAttrs (n: option: {
  inherit (module) _file;
  pos = unsafeGetAttrPos n subtree;
  options = option;
}) subtree
```

and `mergeOptionDecls` then forces it:

```nix
declarationPositions = res.declarationPositions
  ++ (if opt.pos != null then [ opt.pos ] else [ { file = opt._file; line = null; column = null; } ]);
```

The `if opt.pos != null` test forces `pos`, and `mergeOptionDecls` runs for
every option on the path to `config`. **Evaluating a NixOS configuration reads
the source line and column of every declared option.** This landed in
nixpkgs#249243 ("nixos/modules: Add declarationPositions", merged by roberth).

Consequences, in order of severity:

1. You cannot hash `Expr`s modulo position and call it sound. rust-analyzer's
   early-cutoff advice is explicit that it works "if you don't store positions
   in the AST"; Nix stores a `PosIdx` on nearly every node *and* exposes it to
   the language.
2. Inserting one line at the top of a file shifts `PosIdx` for every
   declaration below it in that file, and those entries are genuinely
   invalidated. Fan-out is limited to the edited file, which is acceptable.
3. **Reformatting is the pathological case.** Running a formatter over
   `nixpkgs/lib` or over the ix module tree shifts every position in every
   touched file and invalidates every option declaration in them. An
   incremental evaluator will be no faster than a cold one on a
   tree-wide reformat, and it must not claim otherwise.
4. `PosIdx` is a `uint32_t` offset into a global `PosTable`
   (`src/libutil/include/nix/util/pos-idx.hh:16`). Re-parsing one file appends
   to that table, so the table grows monotonically across a daemon's lifetime.
   Bounded by edits, not by evaluations, but it is a slow leak that needs a
   compaction story eventually.

The design answer is to treat an `Expr`'s position as an input that only
position-observing thunks read, so ordinary values get early cutoff on
whitespace and option declarations do not. That is correct and it is cheap,
and it means the honest claim for this feature is "fast for edits that change
what a config says, no faster for edits that only move where it says it."

### Edit classes, and what each costs

| edit | expected behaviour |
| --- | --- |
| change a string or number in one host's config | re-force that option and its dependents; the target case |
| add a comment or blank line to one host's config | re-force option declarations below it in that file; positions moved |
| add a module to `imports` | re-force the module fixpoint for that host; large, unavoidable |
| bump a flake input | high-durability version moves, everything dirties, no reuse |
| reformat the tree | no reuse; document this as a known non-goal |
| edit a file nothing imports | zero re-forcing, and this is the case that proves the tracking works |

That last row is the phase-1 smoke test and it should be the first thing that
passes.

## Prior art, and what transfers

### Salsa (the closest working system)

`https://salsa-rs.github.io/salsa/reference/algorithm.html`,
`https://salsa-rs.github.io/salsa/reference/durability.html`, and
"Durable Incrementality", 2023-07-24,
`https://rust-analyzer.github.io/blog/2023/07/24/durable-incrementality.html`.

Salsa's model: a database tracks a single revision, incremented on every input
`set`. Tracked functions memoise their return value together with the other
tracked functions they read. On invocation in a new revision, dependencies are
checked and the function re-executes only if one may have changed. Two graph
traversals per top-level query: forward to the inputs, then backward
propagating changes, stopping wherever a result is unchanged despite a changed
input.

**Transfers directly:**

- Early cutoff as the source of the win, not memoisation. Salsa's own framing.
- Durability as a version vector, with derived durability the minimum over
  immediate inputs. Maps cleanly onto locked flake inputs versus the edited
  tree, and the ix case is cleaner than rust-analyzer's because `flake.lock`
  makes the high-durability set explicit rather than heuristic.
- Lazy invalidation. Change an input, bump a counter, do nothing else; the work
  happens when a query is next requested. A daemon serving twelve hosts wants
  exactly this, because an edit typically only matters to one of them.
- The firewall pattern, which is the justification for Decision 2.

**Does not transfer:**

- Salsa's inputs are `set` by an outer loop that is not itself incremental. Nix
  has no such loop: file reads happen *inside* evaluation, discovered
  dynamically. So Nix needs monadic dependencies (dynamic, discovered during
  execution), which in Build Systems a la Carte terms means a suspending
  scheduler with verifying traces rather than Salsa's arrangement.
- Salsa memoises whole return values of Rust functions with known types. Nix's
  values are lazy graphs with functions in them; there is no "return the
  memoised value" that does not immediately hand out a thunk.
- Salsa's `cancel_others` uses `Arc::get_mut` to get exclusive access before
  mutating an input, guaranteeing no reader observes a torn revision. A
  persistent Nix evaluator with parallel eval (lever B) needs an equivalent and
  does not have one. This is a real gap and I would put it in phase 2's risk
  list.

### Adapton and demand-driven incremental computation

Hammer, Khoo, Hicks and Foster, PLDI 2014,
`http://matthewhammer.org/adapton/adapton-pldi2014.pdf`; and Nominal Adapton,
Hammer et al., OOPSLA 2015, `http://www.cs.tufts.edu/~jfoster/papers/nominal-adapton.pdf`.

Adapton is the closest thing in the literature to "incremental evaluation of a
lazy language", because it is built on call-by-push-value with explicit thunks
and its unit of reuse *is* the thunk. Its demanded computation graph contains
only what was demanded, split into an eager dirtying phase (walk backward from
a mutated reference, mark edges dirty, stopping at already-dirty edges) and a
demand-driven propagation phase (walk forward from a forced thunk through dirty
edges, comparing each dependency's current value against the edge label,
re-evaluating only on mismatch). The invariant that makes it cheap is that
dirtiness is transitively closed in both directions, so neither phase revisits
work.

**Transfers:** the two-phase structure, the bidirectional edges, and the
observation that a lazy evaluator already computes the demanded subgraph, so
you get Adapton's central optimisation for free. Also the practical detail that
incoming edges live in weak hash tables so the GC removes dead ones, which
maps onto Boehm's disappearing links and is how a daemon should hold its
reverse index if it is not to pin the entire heap.

**Nominal Adapton is the more important paper for Decision 3.** Its thesis is
that structural matching of computations across runs is not good enough and
that first-class *names* are needed, because a structurally-derived identity
changes when anything upstream changes, so an insertion in a list defeats reuse
for the whole suffix. Their fix is programmer-assigned names with a `fork`
operation for deriving fresh ones deterministically. Nix's analogue of a name
is the attribute path: `config.services.foo.enable` names a computation
independently of where in a file it is written or what shifted above it. **This
is an argument for keying tracked entries on attribute paths rather than on
`Expr` pointers or content hashes**, and it is a genuine design fork I have not
resolved. Attribute paths are stable under edits in a way pointers are not, but
they are not unique (the same value reachable by two paths) and they do not
exist for values inside functions. My inclination is to key on `Expr *` in the
daemon and treat the attribute path as a *hint* for the on-disk phase if it is
ever built, but somebody should argue the other side before phase 2 commits.

### Build Systems a la Carte

Mokhov, Mitchell and Peyton Jones, ICFP 2018 and JFP 2020,
`https://simon.peytonjones.org/assets/pdfs/build-systems-jfp.pdf`.

Provides the vocabulary. The relevant rows:

- **Dirty bit** (Make, Excel): cannot do early cutoff. Rejected; this is what
  "the fingerprint is the whole key" amounts to today.
- **Verifying traces** (Ninja, Shake): store the key, the hash of its value,
  and the hashes of its dependencies. Supports dynamic dependencies,
  minimality and early cutoff. **This is what to build.** Their `verifyVT` is
  exactly step 3 of the invalidation algorithm above.
- **Constructive traces** (Bazel, CloudBuild): also store the value, so results
  can be shared between machines. This is Option C, and the paper's point that
  the client can compute the key and dependency hashes locally then make one
  server call is the right shape for a shared eval cache if we ever want one.
- **Deep constructive traces** (Buck, **Nix**): hash only the terminal inputs,
  skip intermediates. The paper explicitly notes this "cannot support early
  cutoff", and places Nix in that row. Nix's derivation layer is a deep
  constructive trace system; that is why a changed comment rebuilds 31 paths
  even though 20,430 are byte-identical. Worth being clear that this project
  does not fix that, and that fixing it is what content-addressed derivations
  are for.
- **Verifying step traces** (Shake's actual implementation): store built-time
  and changed-time per key plus dependency keys without hashes, which is
  smaller and preserves early cutoff. Given the memory arithmetic in
  Decision 2, this is worth costing as an alternative to storing dependency
  hashes: a shared monotonic revision number per evaluation plus a per-entry
  `built`/`changed` pair is 8 bytes where a hash list is much more. It loses
  reuse only in the case where an input changes and changes back.

### The three stalled Nix designs

The most useful section, because the reasons they stalled are the constraints
this design has to satisfy.

**1. NixOS/nix#4511, "Cache the result of getFlake"** (thufschmitt, opened
2021-02-02, closed 2026-05-18). +1397 -1090 across 28 files. Extended the
existing CLI-level cache into the evaluation loop so that
`(builtins.getFlake foo).bar.baz` was cached, using "special attrsets that
query the cache transparently" on attribute access.

Why it stalled: **overhead on the uncached path.** First benchmark, cache
disabled, was 2.982s against master's 2.152s, a 39% regression. After "a lot of
struggle and rewriting the cache partially from scratch" he got cold and
disabled cache to 0-10% over system Nix (2.366s and 2.279s against 2.183s).
That was still judged not good enough, the PR went stale in August 2021, and it
was closed five years later with "stale: re-open and re-base as needed."
roberth's suggested alternative in #6228 was to add a new *thunk type* instead
of touching attribute access, "so the performance overhead only applies to
cacheable values"; thufschmitt had tried that and reported it was no faster,
attributing the slowdown to the surrounding refactor rather than anything
fundamental.

Constraint for us: **the uncached path must not regress.** A design that taxes
every attribute access or every thunk allocation will be rejected for the same
reason, and Decision 2 exists to satisfy this. Our advantage is that a daemon
lets us put the cost on the boundary set rather than on the hot path, and that
we control our own fork, so a 5% uncached regression is our call to make rather
than upstream's.

**2. NixOS/nix#6228, "Persistent evaluation cache primop"** (roberth, opened
2022-03-10, still open, never implemented). Proposed memoising roughly
`f: j: import f j`, keyed on a store path and JSON-serialisable arguments,
returning a special "cached-eval-thunk" that expands into a scalar or into an
attrset of further cached-eval-thunks. Explicitly designed to avoid serialising
functions by restricting both arguments and results to serialisable data.

Why it stalled: **nobody defined the purity mode it requires.** kamadorueda
immediately found the soundness hole (a cached function can return a path
interpolated from a mutable location outside the store, and the cached result
then outlives the file). roberth's answer was an "even stricter pure mode;
perhaps 'referentially transparent mode'" that throws on
`addToStore(mutablePath)` and similar, with the exception triggering normal
evaluation. He then wrote "I think I'm pretty close, but I can't make a
confident claim without more research" and "I'll stop responding for a while
because of priorities." Nobody picked it up in four years.

Constraint for us: **the purity boundary is the real work, not the caching.**
Our escape is that we control the deployment: run the daemon under `pure-eval`
and refuse impure requests outright, rather than inventing a third purity mode
that has to be sound against adversarial expressions. That converts a research
problem into a configuration decision. It is the single biggest reason this
project is more tractable for us than for upstream.

**3. tweag/epcb, "Evaluation purity and caching builtins"** (Silvan Mosberger,
repo created 2023-07-13, 3 commits, never submitted as an RFC). Proposed
`builtins.pureImport` to establish a pure environment with an explicit allowed
path set, and `builtins.cachedImport file args` keyed on the hash of all
accessible paths plus the serialised arguments. Also proposed
`builtins.lazyUpdate` (a `//` that does not force the left side when the
attribute is on the right, NixOS/nix#4090).

Why it stalled: **the path representation defeated it.** The draft introduces a
`RelativeNixPath` value type, works through its operations, and then contains
the author's own note in the committed text: "FIXME: Okay this is not doable, we
need to keep the same path value type. We should just change the behavior of
builtin functions when pure mode is enabled." The document has unresolved
questions in the body ("`builtins.path` needs to be replaced with something
better", `allowedPaths` specified twice with different types) and was never
opened as an RFC.

Constraint for us: **the input identity for a file read is the hard part, and
lazy trees is the answer the draft was groping for.** epcb needed a path type
that names a file relative to a content-addressed root so a read can be a cache
key; the fork already has source accessors doing that under `lazy-trees`. This
is the design that most directly validates making lazy trees a prerequisite.

**Honourable mentions**, both relevant and neither a full design:

- **edolstra's `eval-cache` branch**, referenced from NixOS/nix#4279: "It's
  still a WIP because the performance isn't on par with the current cache."
  Same failure mode as #4511.
- **NixOS/nix#4279** itself, "Cache evaluation for eval and flake check", where
  roberth notes in a team meeting that "we don't yet store evaluation errors in
  the eval cache," which is a correctness requirement our design inherits: a
  tracked entry must be able to cache a *failure*, and `Value` already has a
  `tFailed` internal type holding an `std::exception_ptr` (`value.hh:431`) that
  makes this natural in-process and impossible on disk.

Common thread across all three: **each tried to add caching to a one-shot
process, and each died on the overhead of doing so.** None of them tried
keeping the process alive. That is the gap this design occupies, and the fact
that three competent attempts converged on the same wall is the strongest
argument for going around it rather than through it.

### Nickel's incremental evaluation skeleton

nickel-lang/nickel#2484, "feat(experimental): skeleton of an incremental
evaluator", opened 2026-01-07, merged 2026-05-20, +638 -122 across 17 files,
released in 1.17.0. Tracking issue #1589 (opened 2023-09-08), formalisation
issue #1650, decoupling issue #1649, and the earlier `Cache` trait refactor
PR #916 (merged 2022-12-06).

**What they merged**, from the PR description and the source of
`core/src/eval/semantic_hash.rs` and `core/src/eval/cache/incremental_ng.rs`:

- Extra fields on thunks to hold semantic hashes, feature-gated because it
  increases thunk size.
- A `Cache` implementation (`IncrementalCache`) that wraps the normal
  call-by-need cache and additionally looks up or stores thunks that carry a
  semantic hash.
- Hooks in the VM that attach hashes at let-bindings, recursive records, and
  array or record closurisation, that is "almost all of the places where we
  allocate thunks for the first time, coming straight from the original source."
- A `--incremental` CLI flag.
- A three-state cache entry (`Loadable`, `Loaded`, `Recorded`) distinguishing a
  thunk from the previous run, one reused this run, and one newly recorded.

**What it does not do yet.** All three of the load-bearing pieces are
`unimplemented!()` in the merged code, and the PR says so: "The most important
part for the actual incremental evaluator are left unimplemented(), so that
running `nickel eval --incremental` will panic as of today." Specifically:

- `pub fn cui(_v: &NickelValue) -> SemanticHash { unimplemented!() }`, the
  cross-evaluation unique identifier, that is the whole identity scheme.
- `pub fn is_of_interest(_v: &NickelValue) -> bool { unimplemented!() }`, the
  thunk-of-interest selection, that is the whole granularity scheme.
- `IncrementalCache::persist` and `::load`, that is all of persistence.

And the one hashing function that *is* implemented carries this comment:

```rust
// TODO: For now, we're being stupid, and hash the whole environment. What we should do is
// 1. Compute the free variables of each expression of interest
// 2. Only retrieve the free variables as dependencies from the environment
```

**What to take from it.** The `semantic_hash.rs` module documentation is the
best short statement of the identity problem I found anywhere, and it should be
read before phase 2 starts. Its framing: a hashing scheme must satisfy
`CUI(e1) = CUI(e2)` implies `e1` and `e2` beta-equivalent, or the evaluator can
change a program's result; the spectrum runs from fresh random identities
(sound, useless) to an ideal scheme that is not computable; and the trade-off is
that a more general scheme equalises more terms but costs more to compute, which
"can nullify the benefits or heavily penalize cases with a lot of changes."

**What to take as a warning.** A funded language team spent from September 2023
to May 2026, two years and eight months, and shipped a skeleton that panics
when enabled. They have a simpler language than Nix, a `Cache` trait boundary
they spent a separate PR creating, no Boehm GC, no store, and no NixOS module
system. If their pace is any guide, the estimate at the end of this document is
optimistic.

### Snix, which is not prior art here

Snix's generator refactor (`https://snix.dev/docs/components/eval/vm-loop/`,
cl/8104 and cl/8148) restructured its VM into an outer loop over bytecode and
generator frames so that deep recursion does not grow the call stack. Their own
documentation is explicit that this uses `async` "notably without introducing
asynchronous I/O or concurrency in `snix-eval` (the complexity of which is
currently undesirable for us)."

So it buys a constant stack, not incrementality. It is worth knowing about for
exactly one reason: the same restructuring would be the enabling refactor if we
ever wanted to *suspend* a Nix evaluation and resume it, which a suspending
scheduler over verifying traces technically wants. In our design we get
suspension for free from the fact that the whole thing runs in one process and
laziness already provides demand-driven ordering, so we do not need it. Do not
count Snix as evidence that anyone has solved this.

## Staging

Four phases, reordered from the brief's sketch. The change is that a
**read-set-keyed fingerprint for the existing cache** is promoted to phase 2,
ahead of the persistent evaluator, because it captures most of the twelve-node
win with none of the daemon's cost and it is a natural consumer of phase 1's
output.

### Phase 0: the ceiling (done)

The measurements at the top of this document. What it bought: the knowledge
that 99.85% of the output is unchanged, and the knowledge that the existing
cache is 130x or nothing.

### Phase 1: read-set instrumentation, no caching

**What it buys:** the number that decides whether phases 2 through 4 are worth
anything, plus a provenance trace that is independently useful for answering
"why did this rebuild".

**Build:** a tracked-entry stack in `EvalState` and a recording hook. Push an
entry when forcing an `import` result, a `config.<path>`, an `options.<path>`,
or a `pkgs.<name>.drvPath`; record every source-accessor read, directory
listing, position observation and store query into the innermost active entry;
pop and emit. Output a trace file of
`(entry key, read set, cpuTime attributable, thunks allocated)`.

The fork already has `src/libexpr/eval-profiler.cc` (361 lines) with a
sampling and tracing profiler, which is where cpuTime attribution should hang
rather than being rebuilt. Nothing in this phase changes evaluation semantics,
so it can be gated by a setting and left in tree permanently.

**Measure, all four:**

1. **Invalidation fraction, weighted by eval time.** Trace a baseline, apply a
   realistic edit (a string change in one host's config; separately, a comment
   added), trace again, and report what share of attributable cpuTime sits in
   entries whose read set hashes changed. Report it with lazy trees off and on.
2. **Whole-tree fan-out.** How many entries have the flake's whole-tree
   accessor in their read set. With lazy trees off this should be nearly all of
   them; if it is still nearly all of them with lazy trees on, the project
   stops here and the finding is worth the phase on its own.
3. **Fork-snapshot prefix.** Cumulative cpuTime before the first read of any
   file in the ix tree, that is, the share of evaluation a zygote would capture
   for free.
4. **Retained set.** Heap still reachable if `(Env *, Expr *)` is pinned for
   every tracked entry, against the live set of a normal evaluation. This is
   the daemon's memory cost and it is currently unknown.

**Success criterion:** all four numbers exist for one ix host and for the
twelve-node case, with lazy trees off and on, and are written into this
document. **Go/no-go:** proceed to phase 2 if eval-time-weighted invalidation
for a single-host string edit is under 25% with lazy trees on. Stop, and do the
cheaper things listed below, if it is over 60%. In between, build phase 2 and
re-decide on phase 3 with its results in hand.

**Estimate:** 4 to 6 weeks, one engineer. Basis: instrumentation of comparable
scope to the existing eval profiler, plus the trace format, plus analysis
tooling, plus a real edit corpus. No semantic changes, so the correctness bar
is low.

### Phase 2: read-set-keyed eval cache

**What it buys:** the twelve-node case. Edit one host, and the other eleven hit
the existing SQLite cache because their recorded read sets did not change.
247s becomes roughly one twelfth of that plus the eleven validations. This is
an order of magnitude on the loop that CLAUDE.md already names as the right
gate (`nix eval` over every host's `config.system.build.toplevel.drvPath`), and
it needs no new evaluator, no invalidation graph and no daemon.

**Build:** replace the whole-tree fingerprint at `src/libflake/flake.cc:1127`
as the eval cache key with a per-entry verifying trace. Store, per cached
attribute path, the read set recorded in phase 1 plus a hash per input. On
lookup, re-hash the recorded inputs; on a full match, serve the cached value;
on any mismatch, evaluate and record the new read set. This is `verifyVT` from
Build Systems a la Carte, unchanged.

**Soundness requirements**, each of which is a real bug if skipped:

- Directory listings must be tracked inputs, or a newly added file that a
  `readDir` would have picked up is invisible to validation.
- Position observations must be tracked inputs, per Decision 3.
- The first evaluation's read set may not cover the second's. Validation
  therefore proves only "the inputs I read last time are unchanged", which is
  sound for a deterministic evaluator reading a subset of a fixed input space,
  and is exactly the argument verifying traces rest on. It requires that
  evaluation be deterministic given its inputs, which `pure-eval` is intended
  to guarantee and which the 569,328-thunk variance between baseline runs
  should be understood before relying on.
- Requires lazy trees on, without exception. With whole-tree accessors every
  read set contains the tree and nothing ever validates.

**Success criterion:** twelve-node `nix eval` after editing one host is under
40s, against 247s to 259s today, with the produced `drvPath`s bit-identical to
a cold evaluation for all twelve. The identity check is the criterion, not the
speed; a fast wrong answer is the failure mode this whole feature risks.

**Estimate:** 8 to 12 weeks, one engineer. Basis: thufschmitt's #4511 was
+1397 -1090 across 28 files and consumed roughly two weeks of concentrated
work plus a rewrite before he reported acceptable overhead, and it failed on
hot-path cost that this design avoids by validating once per request rather
than per attribute access. Doubled for the correctness bar, the lazy-trees
interaction and the shadow-comparison harness.

### Phase 3: persistent evaluator

**What it buys:** the single-host case, which phase 2 does not help at all. One
host after a one-line edit goes from 22s to whatever the invalidated fraction
costs plus validation. This is the case that makes evaluation feel free, and it
is the case the brief is really about.

**Build:** the design in Decisions 1 through 3. A daemon owning `EvalState`,
tracked entries retaining `(Env *, Expr *)`, a reverse index from input to
entries held through Boehm disappearing links, Adapton's two-phase dirtying and
propagation, and Salsa durability levels keyed off `flake.lock`. Start by
making `nix repl`'s `:r` incremental rather than by writing a daemon, so that
the invalidation logic is exercised interactively before any protocol exists.

**Success criterion:** three of them, in order.

1. Editing a file that nothing imports re-forces zero entries. (Smoke test; if
   this fails the tracking is wrong.)
2. Changing one string in one host's config produces a bit-identical `drvPath`
   to a cold evaluation, in under 3s.
3. Shadow mode over a week of real edits on a dev node reports zero
   discrepancies between the daemon's answer and an asynchronous cold
   evaluation.

**Estimate:** 6 months to correct on the ix tree, plus 3 months to trusted as
the default. Basis and caveat in the estimate section below.

### Phase 4: on-disk or shared persistence

**What it buys:** cold start, and sharing between CI runners. Only worth
building if phase 3 shows warm-up dominating, or if the CI fan-out story needs
it. This is Option C and constructive traces, and it inherits epcb's unfinished
purity work.

**Do not estimate this yet.** Decide with phase 3's numbers.

### Phase 1.5, conditional: the zygote

If phase 1's fork-snapshot prefix is worth more than about 40% of evaluation,
build it immediately and reconsider whether phase 3 is needed. Two weeks,
Linux only, needs Boehm fork handlers. Listed out of order because its
existence is contingent on a measurement, and because the point of measuring is
to be allowed to skip the expensive thing.

## Cost estimate and its basis

**Total: 12 to 15 months of one engineer to have the persistent evaluator as
the default, with the first useful speedup landing at 3 to 4 months.**

| phase | estimate | first useful result |
| --- | --- | --- |
| 1, measurement | 4 to 6 weeks | the go/no-go number |
| 2, read-set cache key | 8 to 12 weeks | twelve-node eval 247s to under 40s |
| 3, persistent evaluator | 6 months, plus 3 to trust | single-host eval 22s to under 3s |
| 4, on-disk | not estimated | cold start and CI sharing |

Basis for the phase 3 figure, which is the one that matters:

- **Nickel is the pessimistic anchor.** Issue #1589 opened 2023-09-08, skeleton
  merged 2026-05-20: 32 months elapsed, for a merged artifact whose identity
  scheme, granularity scheme and persistence are all `unimplemented!()`. That
  is not 32 months of full-time work, but it is 32 months of a competent
  language team not finishing.
- **We remove from scope the two things their `unimplemented!()`s are.** A
  persistent process makes cross-run identity into pointer identity and deletes
  persistence entirely. If Nickel's remaining work is mostly those two, our
  scope is genuinely smaller, and 6 to 9 months is defensible against their 32.
- **thufschmitt's #4511 sets the overhead bar** and cost roughly two weeks plus
  a from-scratch rewrite to get within 0-10%, which he still judged
  insufficient. Our advantage is owning the fork.

**Most likely to be underestimated**, in order:

1. **Soundness debugging.** A stale cached value producing a wrong `drvPath` is
   a silently wrong deploy. Finding those requires shadow mode (evaluate cold
   asynchronously, compare, alert), and building and operating that is real
   work with an ongoing compute cost. I have budgeted 3 of the 9 months for
   this and I would not be surprised if it were 6.
2. **Memory, because it is unmeasured.** If pinning `(Env *, Expr *)` for 10^5
   entries retains most of the 3.5 GB, a twelve-host daemon is tens of
   gigabytes, and on a node that also carries VMs that is not deployable. The
   design then needs per-host worker processes with eviction, which is a
   different and larger project. Phase 1 measures this precisely so this risk
   can be priced before phase 3 is funded.
3. **The module system's position dependence.** Found and cited, not
   quantified. If option declarations are a large share of eval time, then the
   comment-added case, which is the demo everyone will ask for, is the *worst*
   case rather than the best, and the feature's story has to be told
   differently.
4. **Daemon lifecycle.** One address space for twelve hosts means one crash
   loses twelve hosts' work, which argues for supervisor plus workers, which
   means the invalidation state has to be shared or partitioned. I have not
   designed this and it is the part of Decision 1 I am least confident in.
5. **Interaction with parallel evaluation.** Lever B landed parallel eval.
   Salsa needed explicit cancellation of all readers before mutating an input
   to prevent a torn revision (`cancel_others` via `Arc::get_mut`); we have no
   equivalent, and a concurrent read during invalidation is a data race with a
   silent wrong answer as its symptom.
6. **Upstream merge tax, forever.** The fork is 47 patches on
   `2.34-maintenance`. A structural change to libexpr conflicts with upstream
   libexpr work at every megamerge, permanently. This is the strongest argument
   for keeping the diff boundary-shaped and small, and for pushing phase 2
   upstream if it works, since it is the piece with a plausible path to
   acceptance.

## What would make this a bad idea

Four conditions, any one of which should stop it.

1. **Phase 1 reports high invalidation.** If a single-host string edit dirties
   more than 60% of eval-time-weighted entries even with lazy trees on, there
   is nothing to reuse. Stop after phase 1. Cost of finding out: 4 to 6 weeks,
   which is the cheapest thing in this document.
2. **The actual pain is deploy latency, not eval latency.** 22s of a 74-minute
   deploy is 0.5%. If the complaint being answered is "deploys are slow", this
   project is the wrong one, and the right one is the deep-constructive-trace
   problem Build Systems a la Carte names: Nix's derivation layer has no early
   cutoff, so 31 changed paths cascade into rebuilds of everything downstream
   even when 20,430 paths are byte-identical. Content-addressed derivations
   attack that; incremental evaluation does not.
3. **Nobody can staff a year.** The phases are independently valuable
   specifically so this can be true and the work still pays, but phase 3
   half-built is worse than not started, because a persistent evaluator that is
   sometimes wrong is a liability.
4. **Fork divergence costs more than eval speed.** If every megamerge already
   hurts, adding a structural libexpr change makes it permanently worse.

## The cheaper thing, if the answer is not to build it

In descending value per week, and the first two are worth doing regardless.

1. **Turn on lazy trees and narrow the fingerprint.** Already in the fork
   behind a setting. Excluding files that no evaluation reads from the
   fingerprint turns some cache misses into hits with no new machinery, and it
   is a prerequisite for everything else here anyway.
2. **Phase 2 on its own.** The read-set-keyed cache key is the best value in
   this document: 8 to 12 weeks for an order of magnitude on the twelve-node
   gate, no daemon, no invalidation graph, and a plausible upstream story. If
   only one thing gets built, build this.
3. **Evaluate all hosts in one process.** Amortises nixpkgs across twelve hosts
   without any caching at all, which is the daemon's main win minus the daemon.
   Partly current practice; making it the enforced gate costs days.
4. **The zygote, if phase 1 says the prefix is large.** Two weeks, Linux only.
5. **Accept 22s and spend the effort on the deploy path instead.** The
   defensible position if condition 2 above holds.

Note that cutting evaluation work rather than caching it has already been tried
and is exhausted: lever 5 (cut the module count) had a 2.4% ceiling and
delivered 0.00s. Do not re-litigate that.

## Open questions for whoever builds this

1. Key tracked entries on `Expr *` or on attribute paths? Nominal Adapton's
   argument for names is strong and attribute paths are Nix's names. Unresolved
   above; decide before phase 3.
2. Verifying traces (dependency hashes) or verifying step traces (built and
   changed revision numbers)? The latter is much smaller per entry and loses
   reuse only when an input changes and changes back. At 10^5 entries either
   fits, but if the boundary set grows this decides the memory budget.
3. What happens to a tracked entry that threw? `Value` has `tFailed` holding an
   `std::exception_ptr` (`value.hh:431`), so caching a failure is natural
   in-process. NixOS/nix#4279 records roberth noting the flake cache does not
   store errors and that this matters for `nix flake check`. Decide whether the
   daemon caches failures, and whether an error's *message* (which contains
   positions) is part of the cached value.
4. How does the daemon notice store changes, including a GC that removes a
   cached `drvPath`? The flake cache re-checks `.drv` existence; a daemon needs
   the same over a longer window plus a story for IFD outputs appearing.
5. Does the 569,328-thunk variance between two identical-result baseline runs
   indicate a nondeterminism that breaks verifying traces? Understand it before
   phase 2 ships.

## What I did not verify

Stated as plainly as the rest, because these are the places this document could
be wrong.

**Taken from the brief, not re-measured:** the 24.85s / 22.17s / 22.36s
timings, the 33,846,982 / 33,277,654 / 33,846,983 thunk counts, the three
`drvPath`s, the 20,461 / 31 / 20,430 closure comparison, the 1.74s to 1.94s
against 247s to 259s eval-cache figures, and the "about 3.5 GB" allocation for
one host evaluation.

**Read from source but not compiled or asserted:** that `sizeof(Value)` is 16.
The bit-packed `ValueStorage` specialisation has exactly one member
(`Payload payload`, a `std::array<uint64_t, 2>`), is `alignas(16)`, and derives
from an empty `ValueBase` under EBCO, so 16 follows; I did not add a
`static_assert` and build it. All the byte arithmetic in Decision 2 depends on
this.

**Not measured at all, and all four are phase 1's deliverable:** the
eval-time-weighted invalidation fraction for any edit; the whole-tree fan-out
with lazy trees on; the fork-snapshot prefix; the retained set of a persistent
evaluator.

**Read only as commit subjects and one setting declaration:** that lazy trees
in this fork yields per-file source accessors for flake inputs in the way this
design needs. I read `lazy-trees` at `eval-settings.hh:299` and the subjects of
`ce4e55576` and `4f436aade`. I did not trace a file read through to confirm the
accessor granularity, and the entire design rests on it.

**nixpkgs claims:** `lib/modules.nix` was read on GitHub master, not against
the revision ix pins. I verified that `mergeModules'` sets
`pos = unsafeGetAttrPos n subtree` per option and that `mergeOptionDecls`
forces `opt.pos` via `if opt.pos != null`, and that `declarationPositions`
landed in nixpkgs#249243. I did **not** verify that these are on the path to
`config.system.build.toplevel` specifically, nor measure what share of
evaluation they represent.

**Nothing was executed against ix.** No evaluation was run from this host; I
did not attempt to reach `hil-compute-1`, and no dev node was touched or
claimed.

**Prior art read at varying depth.** Nickel: merged source of
`core/src/eval/semantic_hash.rs` and `core/src/eval/cache/incremental_ng.rs` at
tag 1.17.0, plus the PR body and commit list; not run. Salsa: the algorithm,
durability and database-runtime reference pages, plus the 2023 blog post; I did
not read the implementation. Adapton and Nominal Adapton, and Build Systems a
la Carte: read via search extracts of the PDFs, substantial but not end to end;
the quoted claims about verifying traces, deep constructive traces and Nix's
placement in their table are from the papers' own text. Snix: the VM loop
document and the generators rustdoc. Nix issues #4511, #6228, #4279, #11322 and
the tweag/epcb README: read in full, including comment threads.

**Possibly missed:** anything newer than #4511's 2026-05-18 closure. I searched
for upstream incremental-evaluation designs and found the three above plus
edolstra's `eval-cache` branch (which I did not read, only the issue comment
describing it as "still a WIP because the performance isn't on par with the
current cache"). A 2026 upstream design could exist that my searches did not
surface.
