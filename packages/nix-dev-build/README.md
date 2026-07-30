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

## The rebuild is serial, so load only matters once the cores run out

Timing `ninja` for the same `eval.cc` edit across a load sweep on 18 cores, with
`real` over `user` as the contention signal:

| load | real | real / user |
| --- | --- | --- |
| 13.2 to 18.4, eight runs | 6.36 to 6.80s | 1.04 to 1.05 |
| about 25, two runs | 13.06s, 15.78s | 1.52, 1.62 |

Below the core count the wall clock is flat and the process is barely waiting.
Above it the same work takes 2.4x longer and the ratio jumps, because the
rebuild needs exactly one core: forcing `ninja -j1` costs almost nothing, 7.03s
and 7.34s against 6.36 to 6.80s at the default. So a jobs flag earns nothing on
a single-file edit and only pays on a cold or wide rebuild.

`user` also inflates under oversubscription, 6.15s to 9.76s, so the same compile
burns more processor cycles rather than only waiting longer. That is cache and
memory bandwidth contention, not scheduling alone.

Two consequences for anyone quoting a number from this tool. Report load beside
it, since the same edit is 6.5s or 16s depending on which side of the core count
the box is. And discard the first run: another session measured a 34% first-run
penalty, 9.06s against 6.77s steady, from cold page cache on the object and the
dylib.

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
