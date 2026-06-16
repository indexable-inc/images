# lib/services: service helpers

`lib/services/` holds three home-manager / systemd helpers that live outside
`modules/` on purpose: they are home-manager modules or pure attrsets, not NixOS
modules, so the [discovery](../discovery/overview.md) walk must not sweep them
into `nixosModules`. `lib/default.nix` imports them directly
(`lib/default.nix:84-89`, `172`) and the flake exposes the two home-manager ones
as `homeModules.portable-services` / `homeModules.mutable-json`
(`flake.nix:286-289`).

## portable-services.nix

One declarative spec rendered to a native launchd agent on macOS and native
systemd user units on Linux (`lib/services/portable-services.nix:1-22`). Write
the service once instead of two hand-synced schemas; the transforms are pure
functions of a fully-defaulted spec so they can be golden-tested.

Public surface (`lib/services/portable-services.nix:355-366`):

- `serviceSubmodule`: the per-service option type
  (`lib/services/portable-services.nix:40`). The portable subset
  (`enable`, `description`, `command`, `environment`, `workingDirectory`,
  `runAtLoad`, `interval`, ...) maps onto both init systems; platform-specific
  keys go through the `launchd.config` / `systemd.service` escape hatches, merged
  last so they always win.
- `toLaunchdConfig svc` / `toSystemdUnits svc`: the pure transforms
  (`lib/services/portable-services.nix:192`, `249`).
- `homeModule`: the home-manager module exposing
  `services.portable.<name>` (`lib/services/portable-services.nix:353`). It
  dispatches on host platform: `launchd.agents` on Darwin,
  `systemd.user.services` (+ `timers` for interval services) on Linux
  (`lib/services/portable-services.nix:334-344`). A stable `key` collapses
  duplicate imports so two composed modules can each import it.

`ExecStart`/`command[0]` must be an absolute path (systemd does not search PATH;
launchd is the same in practice). Consumed by `users/andrewgazelka`,
`ci-bars`, and `indexer` home modules (`flake.nix:301-323`).

## systemd-hardening.nix

A plain attrset of baseline `serviceConfig` hardening for long-running network
daemons (`lib/services/systemd-hardening.nix`). Imported as
`ix.systemdHardening` (`lib/default.nix:172`) and merged into a service's
`serviceConfig`, overriding fields per service as needed. It sets
`ProtectSystem = "strict"` (so a service that writes to `/var` must declare a
`StateDirectory`), empties the capability bounding set and `DeviceAllow`, turns
on the `Protect*`/`Restrict*` family, and keeps `AF_INET`/`AF_INET6`/`AF_UNIX`
open so inbound TCP/UDP still works (`lib/services/systemd-hardening.nix:16-41`).

## mutable-json.nix

Declarative-but-writable JSON config: the fallback for app config Nix cannot
deliver read-only because the app also writes the file at runtime
(`lib/services/mutable-json.nix:1-38`). Prefer the app's own managed/policy layer
to ENFORCE a key (a read-only `/etc` file); reach for this only to SEED an
overridable default or own a key in a file the app rewrites.

`homeModule` (`lib/services/mutable-json.nix:43`, exported at `125`) exposes
`home.mutableJsonFiles.<name> = { target, value }`
(`lib/services/mutable-json.nix:91-110`). On activation it reconciles with a
last-applied 3-way merge (the kubectl-apply model,
`lib/services/mutable-json.nix:24-32`): `result = deepMerge(prune(live, last \
new), new)`, so Nix enforces the keys it declares, prunes keys it stops
declaring, and preserves the app's own writes. The previous declaration is
recorded under `xdg.stateHome`; the merge itself is the sidecar
`mutable-json-merge.jq`. Scope is a single declarative owner per file.

The minecraft systemd integration helper `mkMinecraftSyncManaged` and the
`portable-services` model are the two service-shaped wrappers most images touch;
for the minecraft one see [minecraft](../minecraft/overview.md).
