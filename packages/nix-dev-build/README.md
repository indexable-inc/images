# nix-dev-build

Build a nix source checkout the way nix upstream develops it: meson configures
once, then ninja recompiles what changed. `nix build .#nix-ix` recompiles the
whole modular C++ closure in a sandbox for a one-line edit, so it is the wrong
loop for editing the evaluator.

```sh
nix run .#nix-dev-build -- --checkout ~/src/nix
```

From inside the checkout the flag is unnecessary; the checkout is found by
walking up from the working directory.

Measured on an 18 core aarch64-darwin Mac against a worktree of the fork. Every
figure below is one phase timed alone, with the dev shell entry outside the span;
add 0.4 to 0.9s per invocation for that entry.

| step | wall | load |
| --- | --- | --- |
| `meson setup` into an empty build directory | 11.9s | 10 |
| the first build, all 332 targets | 51.3s | 10 rising to 95 |
| rebuild with nothing changed | 0.1s | 13 |

The one-file rebuild is the number that matters, and it is set by which
translation unit you touched. At load 7 to 8:

| touched | ninja |
| --- | --- |
| `src/libexpr/eval.cc` | 7.9s, 7.2s, 7.1s over three runs |
| `src/libexpr/primops.cc` | 8.9s |
| `src/libexpr/nixexpr.cc` | 2.1s |

So quote a range, 2 to 9s for a body edit in libexpr, and name the file if you
quote one number. A body-only edit recompiles that unit, relinks
`libnixexpr.2.34.7.dylib` and regenerates its symbol file, 3 of 10 edges, and
does not relink `src/nix/nix` at all; the change arrives through dynamic linking.

## The rebuild is serial, and contention doubles it. Load average will not tell you

Thirteen timings of the same `eval.cc` edit, taken across an evening on a shared
machine by two sessions, sorted by `real` over `user`:

| real / user | real | reported 1 minute load |
| --- | --- | --- |
| 1.04 to 1.12, nine runs | 6.36 to 9.38s | 13.18 to 45.37 |
| 1.49 to 1.62, four runs | 12.33 to 15.78s | 24.90 to 37.21 |

The ratio separates the fast runs from the slow ones exactly. Load average does
not: the two ranges overlap almost completely, the fastest run of the night sat
at a reported load of 39.24, a 15.78s run sat at 24.90, and three runs at a
reported load of 42 to 45 came in at 6.77 to 7.55s.

That is a property of the instrument rather than a surprise about the machine.
Load average is a decaying one minute mean and this rebuild is a seven second
event, so the number describes the minute that just ended rather than the seconds
being measured. A burst of other work keeps it inflated for a minute after the
burst is over.

What holds up is the mechanism. The rebuild needs exactly one core: forcing
`ninja -j1` costs almost nothing, 7.03s and 7.34s against 6.36 to 6.80s at the
default. So a jobs flag earns nothing on a single-file edit and only pays on a
cold or wide rebuild. When the process gets that core promptly, `real` is within
11% of `user` and the edit costs about 6.4 to 7.5s. When it does not, `real` runs
about 1.5x `user` and the edit roughly doubles.

`user` also inflates when contended, 6.10s to 9.76s, so the same compile costs
more processor time and not only more wall clock. Why is not established, and the
two obvious candidates are not independent on this part. `hw.nperflevels` is 2,
`perflevel0` is 6 cores named `Super` and `perflevel1` is 12 named `Performance`,
with no efficiency tier, and the tiers differ 2x in cache:

| tier | cores | l1d | l1i | l2 |
| --- | --- | --- | --- | --- |
| `Super` | 6 | 128K | 192K | 16M |
| `Performance` | 12 | 64K | 128K | 8M |

So a compile displaced to the lower tier loses half its L1d and half its L2 as a
consequence of moving. Displacement and cache pressure are the same explanation
here, not competing ones; they separate only for the narrower case where the
compile stays on `Super` and other processes evict its lines.

Nothing here measures that the cache delta causes the 1.59x. The available bound
is that forcing the lowest tier with `taskpolicy -b` costs 2.5x in `user`, and
that is an upper bound rather than a measurement because the flag drops scheduling
priority as well as tier. The experiment that would isolate it pins to
`perflevel0` specifically, rather than raising or lowering quality of service,
and runs under contention on a machine somebody has claimed.

So when quoting a number from this loop, report `real` and `user` together rather
than load.

Discard the first run only after an idle gap, which is narrower than a blanket
instruction. A session measured a 34% penalty on a first run taken after a gap,
9.06s against 6.77s steady, and then no penalty at all on three consecutive runs
taken immediately after building, 7.45s, 7.55s and 6.77s. A third set, taken
while building intermittently, showed 10%. So the page cache is warm if you have
just built, and the first number is suspect only when the machine has been doing
something else in between.

An earlier revision of this file claimed the slowdown had a threshold at
`hw.ncpu`, 18 here. That was two samples and a coincidence with the core count.
Four later samples at a reported load of 37 to 41 came in at 7.39 to 12.33s,
which breaks any monotonic story about load, so the claim is withdrawn rather
than rescued.

## Asserting the exit code is not optional here

Every timing above was re-taken with `ninja`'s exit status checked per run, and
every re-run reported `rc=0`. The first versions of these measurement loops did
not check, in two different ways, and both produced numbers that looked fine:

- Parsing `real` out of `/usr/bin/time` without looking at the status. A build
  that fails early still prints a plausible `real`, and it prints a fast one, so
  a broken run enters the table looking like a good result.
- Piping `ninja` into `tail`. The pipeline reports `tail`'s status, so a failed
  build reads as success rather than merely going unchecked. That is the worse
  of the two, because the check appears to be present.

The same shape turned up in four unrelated measurement harnesses in one evening,
including one that compared output hashes across runs and reported nine clean
runs when one had segfaulted, an empty file having a perfectly stable hash. It
seems to be the cheapest defect class here to look for and the easiest to write.

The tool itself does check. `build.rs` matches on the exit status and treats
anything but zero as an error, including death by signal, which is what a
segfaulting compiler produces:

```
$ nix-dev-build --checkout <worktree> --target src/nix/does-not-exist
ninja: error: unknown target 'src/nix/does-not-exist'
nix-dev-build: ninja exited 1
```

## The dev shell is where the dependencies come from

Every invocation re-execs itself under `nix develop <checkout>#default`, so the
compiler, meson, ninja and the libraries nix links against come from the
checkout's own flake. Nothing here carries a dependency list. Warm, that costs
0.41s.

`--shell` picks another `devShells` attribute. `native-clangStdenv` swaps the
compiler. The compiler is baked into `build.ninja` at configure time, so
switching `--shell` against an existing build directory does not switch
compilers; give each shell its own `--build-dir`.

`native-ccacheStdenv` is the shell the fork manual recommends to "drastically
improve rebuild time", and on this tree it very nearly does nothing. Measured
rather than assumed, because the obvious check is the wrong one: `ccache` is not
on PATH in either shell, so its absence proves nothing. The compiler is the
tell, and the shell does route through it, `clang++` resolving to
`ccache-links-wrapper-4.12.1` against `clang-wrapper-21.1.7` in the default
shell.

Then ccache declines almost all of it. A full 332 target build through that
shell left these stats:

```
Cacheable calls:                     17 / 330 ( 5.15%)
  Misses:                            17 /  17 (100.0%)
Uncacheable calls:                  313 / 330 (94.85%)
  Could not use precompiled header: 284 / 313 (90.73%)
```

The tree compiles with a precompiled header, and ccache refuses to cache a
compilation that uses one unless `sloppiness` includes `pch_defines,time_macros`.
So 5% of compilations are cacheable before any of them can hit. Setting that
sloppiness, or building without the precompiled header, is what would make the
manual's advice true; neither is this tool's business, and neither is done here.

Note for anything scripting this loop by hand rather than through this tool:
`configurePhase` and `buildPhase` are stdenv shell functions, and they are not
defined under `nix develop --command bash -c '...'`, which fails with
`configurePhase: command not found`. Drive `meson setup` and `ninja` directly,
as this tool does.

## What it refuses to do

There is no fallback to `nix build`. A ninja failure means the tree does not
compile, and covering that up would waste the reader's next hour.

- A checkout that is not a nix source tree is refused, naming each marker it
  looked for and whether it was there.
- A build directory configured against a different checkout is refused rather
  than rebuilt through, since ninja's output in that case is confusing rather
  than wrong. `--reconfigure` repoints it.
- A build directory meson left in a failed configure is refused the same way.

## Which binary you just built

`--version` does not identify a checkout build. Meson reads `.version`, so every
build of every branch prints a bare `nix (Nix) 2.34.7`, while the packaged
`nix-ix` on this machine prints `nix (Nix) 2.34.7+ix.g8db4d3805919.h6eeae576`.
Two builds of two different branches are indistinguishable by version string.

So the report names the absolute path of the binary and the revision the
checkout is on, which is the only thing that separates one checkout build from
another:

```
built src/nix/nix in 8.6s

  binary   /tmp/worktree/indexable-inc/nix/fastloop/build/src/nix/nix
  version  nix (Nix) 2.34.7
  tree     b20ed84bb
  on PATH  /run/current-system/sw/bin/nix, nix (Nix) 2.34.7+ix.g8db4d3805919.h6eeae576
```

Both binaries are printed whether or not they collide. The contrast is what
stops a measurement being taken with the wrong evaluator, and it is invisible if
only the build is named. When the two report the same string, the report says so
outright, because at that point only the path tells them apart.

The revision is reported rather than compiled in on purpose: writing `.version`
in the checkout would modify a tracked file in someone's working tree.

`--json` emits one document on stdout with the same facts and both version
strings; build progress goes to stderr.

## Not the same lane as nix-ninja-build-nix

`packages/nix-ninja-build` also builds this source incrementally, but it
materializes the packaged source into a scratch directory and turns each
compilation unit into its own content-addressed derivation, on x86_64-linux
only. This tool builds the checkout you are editing, with local ninja, on any
system the dev shell evaluates for.
