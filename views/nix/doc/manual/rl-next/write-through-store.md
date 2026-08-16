---
synopsis: "New `write-through-store` setting publishes local build outputs before the build reports success"
prs: []
---

Setting `write-through-store` to a store URL makes every locally built
derivation copy its outputs to that store after the outputs are registered
valid and before the build goal reports success. A failure to copy fails the
build.

This makes publication synchronous with the build rather than something that
happens afterwards. Once a build reports success its outputs are durable on the
destination, so the local store holds nothing that exists only there, and can be
treated as a cache that is free to prune.

Only local builds are published. Store objects obtained by substitution, and
outputs of builds delegated to a remote builder, are already durable elsewhere
and are left alone.

Failing the build is the point, and it is why this is a build step rather than a
queue. Deferring publication to a queue drained after the build gives up the
guarantee exactly when it matters: a queue that cannot keep up both blocks
whatever gates on it and pins every queued path against garbage collection,
while the builds that filled it have already reported success.

The default is the empty string, which disables write-through publication and
costs one comparison against an empty string per local build.
