# Backporting

Backporting is manual in this fork.

Upstream automates it with a `backport <branch>` label and a
`.github/workflows/backport.yml` workflow. That workflow was removed here along
with the rest of the hosted CI (see [testing](ix/testing.md)), and it had never
run in this fork regardless: it was gated on
`github.repository_owner == 'NixOS'`, so on `indexable-inc/nix` it skipped every
time.

To backport a change, cherry-pick it onto the release branch yourself and
validate it the way anything else here is validated, by running the test suite on
a dev node.
