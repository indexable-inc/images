---
synopsis: "`github:`, `gitlab:` and `sourcehut:` inputs support `submodules` and `lfs` by fetching via Git"
issues: [13571, 14982]
prs: []
---

The forge archive input schemes now accept the `submodules` and `lfs`
attributes. Forge archive tarballs are `git archive` output and never
contain submodule content or Git LFS files, so such inputs previously
either failed with `input attribute 'submodules' not supported by
scheme 'github'` or silently produced a tree with empty submodule
directories. When one of these attributes is enabled, the input is now
constructed as the equivalent `git+https` input (same host, owner,
repository and revision), which supports both features:

```
nix build 'github:owner/repo?submodules=1'
```

behaves like the previously documented workaround

```
nix build 'git+https://github.com/owner/repo?submodules=1&shallow=1'
```

including in lock files: the locked input is recorded as a plain `git`
input, which older Nix versions understand. This also applies when the
flake sets `inputs.self.submodules = true`, which previously broke
consumers of such flakes.

Note that a Git checkout is not always bit-identical to the forge's
archive of the same revision: archives honor the `export-ignore` and
`export-subst` Git attributes, a Git checkout does not. Inputs that
request neither `submodules` nor `lfs` are unaffected and keep tarball
semantics, hashes and the forge API access-token path. Fetching a
private repository with `submodules = true` uses Git's credential
machinery (for example `netrc-file`) instead of the forge
`access-tokens` setting, as `git+https` inputs always have.
