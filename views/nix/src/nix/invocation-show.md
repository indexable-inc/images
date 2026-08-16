R""(

# Examples

* Show what the last recorded invocation did:

  ```console
  # nix invocation show
  invocation 0198a1f42c0b7f3d9e4a5b6c
    command   nix build .#hello
    wall      12.418 s
    eval cpu  1.902 s
    builds    3 (9.884 s of builder time), substitutions 12

       seconds  kind        on              path
         7.104  build       builder-1       /nix/store/...-hello-2.12.drv
         2.780  build       local           /nix/store/...-app.drv
  ```

* Show a specific invocation, by the id it printed, as JSON:

  ```console
  # nix invocation show 0198a1f4 --json
  ```

# Description

`nix invocation show` reports what one finished `nix` command did: how long
evaluation cost, which derivations were built or substituted, how long each
took, and which machine ran it.

Every `nix` command run with the
[`invocation-records`](@docroot@/development/experimental-features.md#xp-feature-invocation-records)
experimental feature enabled writes a record under
`$XDG_STATE_HOME/nix/invocations/` and prints its *invocation id* on exit. That
id is the argument to this command; a unique prefix of it is enough. With no
argument, the most recent record is shown.

The point is to answer questions about a build after it has finished, without
having watched it. `nix store builds` answers the same questions about builds
that are still running.

`--json` prints the whole record: the command line and timings from
`meta.json`, the evaluator statistics from `eval-stats.json` under `eval`, and
one object per build or substitution under `work`, each with `kind`, `path`,
`on`, `startedAtUs`, `endedAtUs` and `seconds`.

The `on` field is `local` for a build that ran on this machine, and the
[machine name](@docroot@/command-ref/conf-file.md#conf-builders) for one that
went to a remote builder.

# Caveats

A derivation that another client was already building is timed from when this
invocation started waiting for it, not from when that build began. The record
is written by the client, and the client does not see the other client's start.

The number of records kept is `keep-invocation-records`, 100 by default. Older
records are deleted when a new invocation starts.

)""
