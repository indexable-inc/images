---
synopsis: "Binary cache compression defaults to `zstd` instead of `xz`"
prs: []
---

A binary cache destination that does not name a compression method now gets
`zstd`. It previously got `xz`.

The omission is the common case, and under `xz` it was expensive in a way that
did not announce itself. Publishing a 256 MiB output to a local `file://` cache,
with no network involved, measured 2.6 MiB/s with `xz` against 579 MiB/s with
`zstd`, a factor of 238. Compression runs inline with whatever produced the NAR,
so on a write path that blocks, the cost lands on the thread that is being waited
on and presents as a hang rather than as a misconfiguration.

`xz` still produces smaller artifacts and is still available; it now has to be
asked for by name, for example `file:///path?compression=xz`. Existing URLs that
already specify a compression method are unaffected.

The measurement used incompressible input, which is `xz`'s worst case. Real
outputs compress, so `xz` does better on them, but single-threaded `xz` remains
one to ten MiB/s on compressible input and the ordering does not change.
