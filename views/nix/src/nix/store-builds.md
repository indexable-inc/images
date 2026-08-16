R""(

# Examples

* Show what the local Nix daemon is currently building or substituting:

  ```console
  # nix store builds
  build  building /nix/store/…-hello-2.12.drv  (pid 12345, user alice)
      because /nix/store/…-app.drv wants it (outputsMissing)
  ```

* Get the same information as JSON, for consumption by another program:

  ```console
  # nix store builds --json
  [
    {
      "drvPath": "/nix/store/…-hello-2.12.drv",
      "storePath": null,
      "outputs": ["out"],
      "type": "build",
      "pid": 12345,
      "startTime": 1720200000,
      "user": "alice",
      "uid": 1000,
      "logFile": "/nix/var/log/nix/drvs/ab/cdef….drv.bz2",
      "machine": null,
      "why": {
        "rootDrvPath": "/nix/store/…-app.drv",
        "chain": ["/nix/store/…-app.drv", "/nix/store/…-hello-2.12.drv"],
        "cause": "outputsMissing"
      }
    }
  ]
  ```

# Description

This command reports the builds and substitutions that are *currently in
progress*, giving global, daemon-independent observability into what Nix is
doing and *why*.

Unlike most `nix store` subcommands, `nix store builds` does **not** connect to
a Nix daemon. Instead it reads the *status directory*
`<store-state-dir>/status` (e.g. `/nix/var/nix/status`), where each active
build or substitution goal writes one JSON file while it is doing real work and
removes it on completion. The directory location is derived from the Nix state
directory, so it honors the `NIX_STATE_DIR` environment variable.

Because each daemon worker is a separate process, a status file can be left
behind if its writer crashes. `nix store builds` treats a file whose recorded
`pid` is no longer alive as stale: it ignores such files and removes them.

Each status file, and each element of the `--json` array, has the following
fields:

- `drvPath`: the derivation being built, or `null` for a substitution.
- `storePath`: the store path being substituted, or `null` for a build.
- `outputs`: the wanted output names (builds only).
- `type`: `"build"` or `"substitution"`.
- `pid`: the daemon worker process building/substituting this path.
- `clientPid`: the requesting Nix client process, if the daemon transport
  exposes it (otherwise `null`).
- `startTime`: when the work began, in seconds since the Unix epoch.
- `user` / `uid`: the client that requested the work, if known (otherwise
  `null`, e.g. for a local store used without a daemon).
- `logFile`: the on-disk build log path (builds only), if build logs are kept.
- `machine`: the remote builder running this build, or `null` when it runs
  here. This file is the only place the answer survives, because the log
  stream a remote build relays back reports no machine of its own.
- `why`: the chain of goals leading to this one:
  - `rootDrvPath`: the top-level goal the client asked for.
  - `chain`: the goals from the root down to this one, root first.
  - `cause`: a best-effort reason this goal exists (e.g. `"requested"`,
    `"outputsMissing"`, `"outputInvalid"`).

This command is only available with the
[`build-status-dir`](@docroot@/development/experimental-features.md#xp-feature-build-status-dir)
experimental feature enabled. That same feature gates the writing of status
files, so both the producer and this consumer must have it enabled.

)""
