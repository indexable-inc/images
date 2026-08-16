# What agents get wrong in this repo

Short claims first; each has a pointer. Verify at the pointer, not by grep
memory.

## Where the work goes: the Rust bytecode VM

The owner's standing priority (2026-08-05) for this repo is the Rust VM
(`rust/nix-eval-rs` + `rust/ix-kernel`): conformance burndown, drv-hash
parity, and incrementality through the memo table. Default new effort
there. The C++ evaluator gets correctness fixes and whatever the fleet
needs to keep running; it does not get new feature investment without the
owner asking, because it is the bridge and the VM is the destination.


## Everything evaluates on the bytecode VM; the bridge decides nothing

Owner's direction (2026-08-06). The invariant, checkable in the code:

- One pipeline: rnix CST -> `compile` -> IR -> VM (`lib.rs` module doc).
  `ir.rs` references nothing from the CST on purpose; a second compiler may
  emit IR, but nothing interprets a tree at runtime. Adding a tree-walking
  path, however small, is a second evaluator and a rejected design.
- Semantics live in the VM: IR ops, `Cont` continuations, `Task` machines.
  A builtin that needs to evaluate more Nix (a `__toString`, a filter, a
  comparator) is a resumable machine the VM drives -- never a callback that
  re-enters evaluation from the host stack. `Coerce` in `print.rs` is the
  worked example; `builtins.path`'s filter walk is the load-bearing one.
- The C++ bridge (`src/nix/rust-eval-session.cc`) is an embedder only:
  host questions in (`NeedPath`), answers out. A semantic decision in the
  bridge is a defect wherever it is found. The test: could the probe
  (`examples/nixpkgs-probe.rs`) stand in for the bridge? If a behaviour
  exists only bridge-side, it is in the wrong place.
- Flake **locking** is bridge machinery, like the fetchers, and is the one
  place the C++ evaluator runs under `eval-backend = rust`. Reading
  `flake.nix`'s `inputs` to produce a lock file walks the input graph, hits
  the registry and writes `flake.lock`; a stand-in would be inventing
  lock-file data, exactly as it would for a fetch. It is scoped to
  `EvalState::LockingFlake`, counted apart as `evaluatorCalls.cppFlakeLock`,
  and what crosses out of it is a lock file that reaches the VM as data. A
  flake's **outputs** always evaluate on the VM, out of `call-flake.nix`
  applied to that data; `rust-nix-eval-gate.sh` section 8 holds that by
  requiring a flake whose outputs use an unimplemented construct to refuse by
  name rather than be answered.
- The VM performs no IO. Every world access is a `NeedPath` variant, which
  is what makes recorded read sets complete and the memo table sound. A
  builtin reaching the world any other way silently breaks incrementality;
  `getEnv` used to and was fixed (see the `NeedPath::Env` comment).
- One implementation per rule: coercion is `Coerce`, store-path computation
  is `storepath.rs`/`nixhash.rs`. A builtin hand-rolling either is a mirror
  that will drift; route through the existing machine or fix the machine.

## Direction: full Rust, ship of Theseus (owner, 2026-08-06)

The terminal state is one Rust binary. The C++ tree is a compatibility
shell being replaced plank by plank, not a peer implementation.

- Given a capability buildable fully in Rust or by wiring C++ to Rust at
  comparable cost, build it in Rust. Bridge wiring is scaffolding that
  will be deleted; pay for it only when Rust-first is genuinely blocked,
  and name what blocks it.
- First shell plank: a Rust-native evaluation driver (a CLI entry over
  `drive_concurrent` plus a Rust `Host` answering store questions for
  real), so overlapped IFD stops waiting on a C ABI batch entry point in
  the C++ CLI.
- Two things are not planks yet. The C++ evaluator stays while it is the
  differential oracle for `lang-diff.sh` and `drv-parity.sh`; it leaves
  when the gates compare against a pinned upstream binary instead. The
  fetchers and flake locking stay bridge-side per the section above until
  Rust replacements pass parity gates of their own, because a second
  fetcher implementation is a second set of answers for a store path to
  differ over. Everything else C++ is a candidate.
- Replacing a plank includes deleting the C++ path it replaces in the
  same change, or naming the ticket that will. Grep for every retired
  name and remove the doc lines, comments and examples that promise it.

## There are two evaluators, and one of them is Rust

- `rust/nix-eval-rs` is a bytecode VM for the Nix language (fixed-width-op
  IR, compiler, explicit-frame lazy stack VM), linked into the `nix` binary
  behind `eval-backend = rust` (experimental feature; default is `cpp`).
  `rust/ix-kernel` is the memo-table kernel beside it. Workstream: ENG-12068;
  active branches `claude/eng-12068-*`.
- A grep for "bytecode" scoped to `src/libexpr` finds nothing and has already
  produced a confident "no bytecode VM exists" answer. Search `rust/` too.
- Equivalence between the backends is behavioral, gated by
  `maintainers/ix/lang-diff.sh` against `maintainers/ix/eval-allowlist.toml`
  (every accepted divergence needs a reason; semantic ones need a human name).
- The ramp to making Rust the default is `maintainers/ix/rust-default-ladder.md`
  (979 lines). That file is the plan of record for the flip; this one is not.
  Read it before proposing a default change or re-deriving the sequencing.
- Only `nix eval` and `nix-instantiate --eval` serve the Rust backend. Every
  other command, `nix build` and `nix-build` and `nix-env` and all flake
  evaluation included, refuses by name through `requireBackendCanServe`
  (`src/libexpr/eval.cc:1316`) even with `eval-backend = rust` set. That
  refusal is the current design. Do not report it as a regression, and do not
  benchmark a "Rust" number from a command that cannot serve Rust.

## Parity bar: byte-exact for hashes, functional for presentation

Owner's direction (2026-08-05): the VM does not have to match cppnix 1:1;
it has to produce the functionally same binary. That splits parity in two:

- Tier 1, byte-exact, non-negotiable: `.drv` ATerm bytes, outPaths,
  drvPaths, and anything that feeds a hash. There, byte-identity IS
  functional identity; a different outPath is a different store path and
  nothing substitutes.
- Tier 2, functional equivalence suffices: error wording, printer and
  render format, warning text, trace shape. Do not spend effort chasing
  byte parity of presentation; lang-diff's error-class comparison is the
  intended bar for fail-as-fail cases. Semantic divergences (different
  values, different failures) still need a human-approved allowlist entry.

The IR carries source positions (ENG-12137), so an error names the line
it happened on and `unsafeGetAttrPos` answers a real one. What still has
none is written down in `maintainers/ix/positions.md`; the short version
is that a set carries ONE origin rather than a position per attribute, so
a set assembled from more than one source -- `a // b` asked about an
attribute only `a` had, `builtins.listToAttrs` -- answers `null` where
cppnix has a record. The rule that keeps that safe is that a derived set
takes the origin of the operand whose values it takes, so an answer is
never a real line belonging to a different attribute. Traces are a
separate gap (ENG-12714): the position is on the error, and this backend
builds no `while evaluating ...` frames above it.

Owner's direction (2026-08-06): speed is the objective and Tier 1 plus
observable semantics are the constraint; nothing else is. Byte-for-byte
matching of cppnix is required exactly where bytes feed a hash and nowhere
else, because chasing it elsewhere forecloses the optimizations this
backend exists for. Concretely: internal representations, evaluation
order not observable through semantics, traversal strategies, string
rendering internals and cache shapes are all free to differ. When a
speedup and Tier 2 byte-parity conflict, take the speedup and record the
difference; when a speedup and Tier 1 or semantic parity conflict, there
is no conflict, the speedup is wrong. A comparison harness that flags a
Tier 2 byte difference as a failure is a harness bug; fix the harness.

## Incremental eval is C++ and has named unsoundness

- `nix eval-persistent --retain` (`src/nix/eval-persistent.cc`) reuses a live
  evaluator across runs. The only accepted correctness evidence is comparison
  against a fresh process; the retained process agreeing with itself proves
  nothing.
- What invalidation reaches and what it misses is edit-class dependent.
  Read `maintainers/ix/read-set-recall.md` before claiming a recall number;
  the 22/22 figure is one edit class, not a general property.
- Direction, not yet built: the durable home for incrementality is the Rust
  VM, where code units are content-addressed CAS objects and `ix-kernel`'s
  memo table is the eval cache (one invalidation story). The C++ retained
  evaluator is the bridge, not the destination.

## Branch model: one branch, never rewritten

- `ix-patched` is the only branch that matters. Ordinary commits, one per
  patch, pushed directly. Upstream moves land as two-parent merges, never a
  rebase. Delta over upstream:
  `git log upstream/main..ix-patched --first-parent --no-merges`
  (both flags load-bearing).
- flake.locks in other repos pin these revs, so force-push is exceptional and
  needs a `refs/pins/<date>-<sha12>` ref for every pinned rev in the same
  operation.

## Merging and shipping

- This repo does NOT allow auto-merge: `gh pr merge --auto --merge` merges
  immediately, silently. Check before arming.
- A fix here reaches the fleet only after ix bumps its nix-src pin and
  deploys. "Merged" is not "running anywhere".

## Build loop

- `nix-dev-build` recompiles one edited file in 2-9s; a whole-package
  `nix build` recompiles the closure. `nix develop --command bash -c
  'configurePhase'` fails (stdenv shell functions undefined); call `meson
  setup` and `ninja` directly.
- A checkout build's `--version` carries no revision. Identify a measured
  binary by store path or file path, never by version string.

## Tests

- Baseline and recording rules are in `maintainers/ix/testing.md` (the lang
  suite baseline is 286 Ok; report counts on the same line as the claim).
- Read a gate's exit status from the gate, never through a pipe. `pre-commit
  run --all-files | tail -25; echo "rc=$?"` reports `tail`'s status, so it
  prints 0 however red the gate is; that line put "pre-commit clean" into
  three merged PRs (#52, #53, #54) while pre-commit was in fact failing on
  six files (ENG-12444). Use `set -o pipefail`, `${PIPESTATUS[0]}`, or no
  pipe at all.
- Committing here fails with "No .pre-commit-config.yaml file was found",
  because the config is generated by the dev shell while the hook is
  installed in the global hook path, so a fresh worktree fires the hook
  before anyone enters a shell. The answer is `PRE_COMMIT_ALLOW_NO_CONFIG=1
  git commit`, and it is not a `--no-verify`-style bypass: it skips nothing,
  because nothing is configured to skip. Run the real gate inside `nix
  develop`. Four agents hit this in one day (ENG-12590), each stalling
  because refusing `--no-verify` correctly left them no sanctioned path.
- `pre-commit` rewrites the tree: clang-format edits files in place. A build
  run after it in the same script is built from a tree matching no committed
  revision, which is how two measurement runs came to describe a revision
  nobody can check out. Run it before the build, or in a separate clone, and
  check `git status` after.

## A setting is not a capability: check the effect

- `nix config show` reports `eval-backend = rust` on a binary compiled
  **without** the Rust evaluator (`-Dnix:rust-eval=enabled` is off by
  default). A gate that reads the setting therefore passes while measuring a
  stub: one lang-diff run scored `mismatch=249` that way. Probe by evaluating
  (`--eval --strict -E 1` must print `1`) before scoring anything, as
  `lang-diff.sh` and `rust-eval-cache-cli.sh` both do.
- The same shape applies to any config the backend takes. `eval-cache-dir` is
  reported whether or not it is wired; the check that means something is that
  objects, rows and witnesses appear under it. An inert setter passes every
  settings-based assertion, and did: making `ixe_set_eval_cache_dir` a no-op
  left `nix config show` unchanged and the store empty.
- A wired cache is not a *harmless* cache. `eval-cache-dir` may change speed
  and nothing else, and four separate things made it change meaning
  (ENG-12540): the memoising path built its VM without the call-depth ceiling;
  the recording host inherited `Host` defaults instead of forwarding them; the
  witness decoder rejected the tag its own encoder writes; and one Ctrl-C was
  memoised, so the interrupted expression answered "interrupted by the user"
  for ever. Every evaluator setting now goes in the memo key through
  `eval::Settings`, whose `fingerprint` destructures every field so a new
  setting will not compile until somebody places it (ENG-12541); every `Host`
  question a wrapper must not answer for itself is a bodiless trait method, so
  a forwarding wrapper that skips one does not compile. There are no
  exceptions left: the seven effects that kept a `NoStore` default -- because
  a leaf host with no store is cppnix's `readOnlyMode` and every test host
  here is one -- lost it in ENG-13107, and that convenience moved to
  `host::host_stubs!`, which a leaf asks for by name and a wrapper must never
  reach for. `resolve_import` is the sole method with a body, being derived
  rather than an effect, and
  `host::trait_shape_tests::the_trait_has_no_default_bodies_to_inherit` parses
  the trait to refuse a second one -- the only part of this the compiler
  cannot enforce, since a default that is never written is not a compile
  error anywhere. What the two per-wrapper guard tests used to remember is now
  a compile error at the wrapper, by name; and `maintainers/ix/cache-semantics-gate.sh` differs along the
  *setting* rather than along the evaluator -- the axis lang-diff structurally
  cannot cover, since both its arms are configured the same way.
- Rebuild what you are about to measure. Editing the Rust library and
  rerunning without `cargo build --examples`, or without relinking `ninja`
  after a change the C++ links against, measures the previous binary. That
  produced a consistent and entirely fake soundness failure once, chased for
  an hour, after a break test left a broken binary behind.
- `git fetch origin <branch>` does not reliably move
  `refs/remotes/origin/<branch>`, so `--is-ancestor` against it can report
  merged work as missing. Use a plain `git fetch origin` before concluding
  anything from a remote-tracking ref.
