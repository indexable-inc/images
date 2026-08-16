---
synopsis: "Forge fetchers: opt-in incremental fetching via the Git protocol"
prs: []
---

The `github:`, `gitlab:` and `sourcehut:` fetchers can now fetch a
revision through the Git smart protocol instead of downloading an
archive tarball from the forge, behind the new `forge-fetch-via-git`
setting:

```
nix flake update --option forge-fetch-via-git true
```

The revision's objects are fetched (shallow) straight into the global
Git object cache that archive tarballs are already unpacked into, and
Nix keeps a per-repository negotiation ref in that cache. Since the Git
protocol only transfers objects missing locally, updating an input
downloads roughly the delta since the previously fetched revision
instead of a full archive of the new revision.

The result is bit-identical to the archive download, so lock files and
`narHash` are unaffected. Revisions whose trees would not export
identically to their archive (repositories using submodules or the
`export-ignore`/`export-subst` Git attributes), and revisions the Git
server refuses to serve, automatically fall back to the archive
download.
