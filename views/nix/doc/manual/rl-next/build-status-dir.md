---
synopsis: "New experimental `build-status-dir` feature and `nix store builds` command"
prs: []
---

Nix can now expose what a daemon is building or substituting, and *why*,
without connecting to the daemon.

With the `build-status-dir` [experimental feature](@docroot@/development/experimental-features.md)
enabled, each active build or substitution goal writes a JSON status file to
`<store-state-dir>/status/` (e.g. `/nix/var/nix/status/`) while it is doing
real work, and removes it on completion. Each file records the derivation or
store path, the wanted outputs, the worker pid, the requesting client's
uid/user, the on-disk build log, and the chain of goals leading up to the
root that the client requested.

The new `nix store builds` command reads that directory directly — it does
*not* connect to a daemon — and reports the builds and substitutions currently
in progress, as a human-readable summary or, with `--json`, as a JSON array.
Stale files left behind by a crashed worker (identified by a dead pid) are
ignored and cleaned up.
