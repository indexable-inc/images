Use the GitHub CLI credential helper for HTTPS pushes when the default helper
would reuse a read-only bot credential:

```sh
gh auth setup-git
git push -u <canonical-remote> <branch>
```

Choose the remote name that points at `indexable-inc/index`, such as `origin` in
the shared checkout or `upstream` in a fork-based clone. Keep the branch tracking
the same remote that received the push.

