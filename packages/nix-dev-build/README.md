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

Measured on an 18 core aarch64-darwin Mac against a worktree of the fork, at a
load average starting near 10:

| step | wall |
| --- | --- |
| `meson setup` into an empty build directory | 11.9s |
| the first build, all 332 targets | 51.3s |
| rebuild after touching `src/libexpr/eval.cc` | 8.6s |
| rebuild with nothing changed | 0.1s |

The one-file number is the one that matters. Add 1.4s to each for entering the
dev shell. A second run of the same steps on a busier machine, load 13 to 24,
took 12s, 93s, 18s and 1s, so treat these as the shape rather than constants.

## The dev shell is where the dependencies come from

Every invocation re-execs itself under `nix develop <checkout>#default`, so the
compiler, meson, ninja and the libraries nix links against come from the
checkout's own flake. Nothing here carries a dependency list. Warm, that costs
0.41s.

`--shell` picks another `devShells` attribute: `native-ccacheStdenv` keeps
compiler output across reconfigures, `native-clangStdenv` swaps the compiler.

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
