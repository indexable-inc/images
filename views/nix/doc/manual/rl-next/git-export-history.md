---
synopsis: "Git fetcher: opt-in structured commit history via `exportHistory`"
prs: []
---

Git inputs (`builtins.fetchGit`, `builtins.fetchTree` and flake inputs of
type `git`) can now opt into exposing the commit history of the fetched
revision as structured evaluation-time data, behind the new
`git-export-history` experimental feature:

```nix
builtins.fetchGit {
  url = "https://github.com/NixOS/patchelf.git";
  rev = "...";
  exportHistory = true;
  historyDepth = 50; # at most this many commits, nearest first; 0 = unlimited
}
```

The result gains a `history` attribute: a list of commits, newest first, in
a deterministic topological order (children always precede parents; ties are
broken by commit hash), where each entry carries `rev`, `parents`, `author`,
`committer`, the full `message`, and the `paths` touched relative to the
first parent. Computing `paths` costs a tree diff per commit, which is
expensive on repositories with large trees (for example nixpkgs); pass
`historyPaths = false` for cheap metadata-only history. This makes
evaluation-time changelog generation and per-subtree release notes possible
without `leaveDotGit`-style nondeterminism: the history is a pure function
of the locked revision, is cached in the fetcher cache, and never appears in
lock files (exactly like `revCount` and `lastModified`, which it
generalises).
