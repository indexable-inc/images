---
synopsis: "New `parallel-compression-threads` setting bounds NAR compression to a fixed thread count"
prs: []
---

`parallel-compression` on a binary cache store has always been a choice
between one thread and every core on the machine: the underlying
libarchive call passed a hardcoded `threads=0`, which asks the compressor
for "as many threads as the hardware has." There was no way to ask for,
say, four threads.

The new `parallel-compression-threads` setting fills the gap. `0`, the
default, keeps today's behavior exactly: `parallel-compression = true`
still means every core, and `parallel-compression = false` still means
serial compression. Setting `parallel-compression-threads` to a positive
number pins the thread count instead, and on its own is enough to turn on
multi-threaded compression, without also setting `parallel-compression`.

This matters on a host that also runs other latency-sensitive work: a
large upload holding every core for the duration of its compression can
starve unrelated processes on the same machine, and until now the only
way to avoid that was to fall back to single-threaded compression
entirely, which is far slower.

Requesting a thread count for a compression method that does not support
it (anything other than `xz` or `zstd`) is a compression-time error
naming the method, not a silently ignored setting.

Measured, publishing a 256 MiB output that compresses 2.3x under zstd, on
a 32-core EPYC 9135 with libarchive 3.8.4, two independent passes:

| threads | MiB/s (pass 1) | MiB/s (pass 2) |
| ------- | -------------- | -------------- |
| unset   | 235            | 235            |
| 1       | 328            | 330            |
| 2       | 605            | 628            |
| 4       | 977            | 1062           |
| 8       | 934            | 1000           |
| 16      | 1080           | 955            |
| 32      | 895            | 829            |

Two things worth knowing from that. The useful range ends around four
threads: beyond it the curve is a noisy plateau, and every-core was the
slowest parallel setting in both passes while occupying eight times the
machine. And `1` is not the same as leaving it unset, because libarchive's
multi-threaded zstd path is roughly 40% faster than its serial one even
with a single thread.

The numbers come from `maintainers/ix/write-through-throughput.sh
--payload elf`.
