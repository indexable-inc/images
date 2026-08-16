# NoFib: Haskell Benchmark Suite

This is the root directory of the "NoFib Haskell benchmark suite".
There are currently two means of running the `nofib` benchmarks:

 * [the `shake`-based build system](shake/README.mkd)
 * [the legacy `make`-based build system](README.make.mkd)

Users are generally encouraged to use the former when possible. See the linked
READMEs for usage instructions.

Additional information can also be found on
[NoFib's wiki page](https://gitlab.haskell.org/ghc/ghc/-/wikis/building/running-nofib).


## Adding benchmarks

If you add a benchmark try to set the problem sizes for
fast/normal/slow reasonably. [Modes](#modes) lists the recommended brackets for
each mode.

### Benchmark runtimes

Benchmark should ideally support running in three different modes:

- `fast`: 0.1-0.2s
- `norm`: 1-2s
- `slow`: 5-10s

You can look at existing benchmarks for how this is usually achieved.
### Benchmark Categories

So you have a benchmark to submit but don't know in which subfolder to put it? Here's some
advice on the intended semantics of each category.

#### Single threaded benchmarks

These are run when you just type `make`. Their semantics is explained in
[the Nofib paper](https://link.springer.com/chapter/10.1007%2F978-1-4471-3215-8_17)
(You can find a .ps online, thanks to @bgamari. Alternatively grep for
'Spectral' in docs/paper/paper.verb).

- `imaginary`: Mostly toy benchmarks, solving puzzles like n-queens.
- `spectral`: Algorithmic kernels, like FFT. If you want to add a benchmark of a
  library, this most certainly the place to put it.
- `real`: Actual applications, with a command-line interface and all. Because of
  the large dependency footprint of today's applications, these have become
  rather aged.
- `shootout`: Benchmarks from
  [the benchmarks game](https://benchmarksgame-team.pages.debian.net/benchmarksgame/),
  formerly known as "language shootout".

Most of the benchmarks are quite old and aren't really written in way one would
write high-performance Haskell code today (e.g., use of `String`, lists,
redefining own list combinators that don't take part in list fusion, rare use of
strictness annotations or unboxed data), so new benchmarks for the `real` and
`spectral` in brackets in particular are always welcome!

#### Other categories

Other than the default single-threaded categories above, there are the
following (SG: I'm guessing here, have never run them):

- `gc`: Run by `make -C gc` (though you'll probably have to edit the Makefile to
  your specific config). Select benchmarks from `spectral` and `real`, plus a
  few more (Careful, these have not been touched by #15999/!5, see the next
  subsection). Testdrives different GC configs, apparently.
- `smp`: Microbenchmarks for the `-threaded` runtime, measuring scheduler
  performance on concurrent and STM-heavy code.

### Stability wrt. GC paramerisations

Additionally, pay attention that your benchmarks are stable wrt. different
GC paramerisations, so that small changes in allocation don't lead to big,
unexplicable jumps in performance. See #15999 for details. Also make sure
that you run the benchmark with the default GC settings, as enlarging Gen 0 or
Gen 1 heaps just amplifies the problem.

As a rule of thumb on how to ensure this: Make sure that your benchmark doesn't
just build up one big data and consume it in a final step, but rather that the
working set grows and shrinks (e.g. is approximately constant) over the whole
run of the benchmark. You can ensure this by iterating your main logic `$n`
times (how often depends on your program, but in the ball park of 100-1000).
You can test stability by plotting productivity curves for your `fast` settings
with the `prod.py` script attached to #15999.

If in doubt, ask Sebastian Graf for help.

## Important notes

Note that some of these tests (e.g. `spectral/fish`) tend to be very sensitive
to branch predictor effectiveness. This means that changes in the compiler
can easily be masked by "random" fluctuations in the code layout produced by
particular compiler runs. Recent GHC versions provide the `-fproc-alignment`
flag to pad procedures, ensuring slightly better stability across runs. If you
are seeing an unexpected change in performance try adding `-fproc-alignment=64`
the compiler flags of both your baseline and test tree.
