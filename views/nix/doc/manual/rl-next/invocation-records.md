---
synopsis: "New experimental `invocation-records` feature and `nix invocation` command"
prs: []
---

A finished Nix command can now be asked what it did, without having watched
it happen.

With the `invocation-records` [experimental feature](@docroot@/development/experimental-features.md)
enabled, every `nix` command writes a record of itself to
`$XDG_STATE_HOME/nix/invocations/<invocation-id>/` and prints that id on
stderr as it exits. The record holds the command line, the wall clock, the
evaluator statistics, and the timestamped `internal-json` event stream.

`nix invocation show <id>` reads one back: evaluation CPU time and allocation
counts, then every derivation that was built or substituted, how long each
took, and the machine it ran on. `nix invocation list` lists the records that
are still kept. A unique prefix of an id is enough, and `last` names the most
recent one.

This complements [`nix store builds`](@docroot@/command-ref/new-cli/nix3-store-builds.md),
which answers the same questions about builds that are still running.

`keep-invocation-records` bounds the directory; it defaults to 100, and the
oldest records are deleted when a new invocation starts.
