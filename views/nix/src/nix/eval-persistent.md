R""(

# Description

Evaluate each *installable* in turn inside a single evaluator, keeping the
parsed expressions, the symbol table and every value forced so far alive from
one evaluation to the next.

A one-shot `nix eval` throws all of that away when the process exits, so two
evaluations of the same host cost the same whether or not anything changed.
Here the second evaluation reuses whatever the first one built and is still
valid. What stays valid is decided by whether an input is locked: an input
pinned in a lock file names its own content and can never go stale, so
everything parsed and forced under it is reused, while a working tree under
edit is refetched for every request so that an edit is never invisible.

With `--interactive`, installables are read one per line from standard input
instead. That is what lets a caller evaluate an attribute, edit a file, and
evaluate the same attribute again in the same process.

One JSON object is written per request, reporting the wall and CPU time it
cost, the thunks it allocated (zero unless `NIX_SHOW_STATS` is set, which is
what enables the counters), how many cache entries were evicted as unlocked,
and the value produced. The value is reported because a warm evaluation is
only interesting if it answers what a cold one would.

# Examples

Evaluate one host twice, editing the tree in between:

```console
$ printf '%s\n' ".#nixosConfigurations.hil-compute-1.config.system.build.toplevel.drvPath" \
    > /tmp/requests
$ nix eval-persistent --interactive < /tmp/requests
```

## Retention (`--retain`)

With `--retain`, each request leaves behind its tracked-entry graph and the
fully forced result of every `derivationStrict` call. The next request for
the same installable diffs the recorded inputs against the tree as it stands,
walks the graph from the changed ones, and answers each clean derivation
boundary from the retained value without forcing its attributes.

This is a measurement prototype, not a cache to trust. Values that cross
into a derivation are tracked by payload provenance at file granularity: a
string literal names the file it was parsed from, a tree version attribute
names the tree, and the consuming derivation carries those as inputs of its
own. On the host edit that previously served 38 of 1,316 splices stale, the
provenance closes all 38. What still escapes is an integer or boolean that
reaches a derivation with no same-file string beside it and no tracked
string operation rendering it, influence through control flow alone, and
`fromTOML`. The `NIX_RETAIN_LOG` environment variable names every splice and
its produced store path so a comparison against a fresh process can attribute
each wrong answer to its entry.

)""
