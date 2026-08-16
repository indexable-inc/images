---
synopsis: Flake support for Jujutsu (jj) working copies
issues: [15651]
---

Nix now understands [Jujutsu](https://jj-vcs.github.io/) working copies that are
not colocated with Git. Previously, evaluating a flake in such a directory (most
notably a `jj workspace add` workspace, which has a `.jj` directory but no
`.git`) fell back to the `path` fetcher, copying the entire working directory,
including build artifacts and untracked files, into the store with no
filtering.

A new `jj` input scheme detects `.jj` directories and shells out to the `jj` CLI
to determine which files are tracked, so only those are copied:

```nix
builtins.fetchTree { type = "jj"; url = "file:///path/to/working-copy"; }
```

Flake references to a local path that resolve to a Jujutsu working copy without a
colocated Git repository are routed to this fetcher automatically. Colocated
repositories (`jj git init --colocate`) continue to use the Git fetcher.

An explicit revision or bookmark can also be fetched:

```nix
builtins.fetchTree { type = "jj"; url = "file:///path/to/repo"; rev = "<commit-id>"; }
builtins.fetchTree { type = "jj"; url = "file:///path/to/repo"; ref = "<bookmark>"; }
```

Two behaviours are worth knowing before pointing this at a repository that is
also usable through Git, because in both cases the two fetchers return a
different store path for what looks like the same tree.

**A new file is part of the source as soon as it exists.** jj snapshots the
working copy on every command and auto-tracks anything not covered by
`.gitignore`, so there is no untracked state to stage out of. Creating a file
changes the flake source immediately, where the Git fetcher would ignore it
until `git add`. Deleting it or adding it to `.gitignore` reverses that.

**Git submodules are not included, and `submodules = true` does not change
that.** `jj file list` reports a submodule as a single entry and cannot
enumerate the files inside it, so this fetcher renders a submodule as absent,
matching a `git+file` input that does not request submodules. The attribute is
accepted and has no effect, as any unrecognised input attribute is; a warning
names the omitted paths on each fetch. Use the Git fetcher for a tree whose
submodule contents have to reach the build.
