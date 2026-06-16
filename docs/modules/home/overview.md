# home

`modules/home/` holds home-manager modules (workstation-facing, not NixOS
system modules). It currently contains one file, `raycast.nix`. Unlike the
service and profile modules, `modules/home/` is NOT auto-discovered:
`discoverModules` only treats a directory with its own `default.nix` as a module,
and `raycast.nix` is a bare file, so the walk yields nothing here (see
[common](../common.md)). It is wired by explicit path as the flake output
`homeModules.raycast` (`flake.nix:295`).

The repo's other home-manager modules (`portable-services`, `mutable-json`,
`andrewgazelka`, `ci-bars`, `indexer`) live outside `modules/` (in `lib/services/`
and under `packages/`/`users/`) and are out of this domain's scope.

## raycast (`programs.raycast.focus`)

`modules/home/raycast.nix` declares Raycast Focus session defaults on macOS.
Raycast stores Focus settings in the `com.raycast.macos` preferences domain. The
title and filter mode are plain strings, but the session duration is a plist
`<data>` value whose bytes are a JSON document; nix-darwin's attrset-to-plist
generator has no `<data>` type, so this module writes every managed key with
`defaults` instead, encoding the duration as JSON-in-data via `/usr/bin/od`
(`raycast.nix:1-9`).

Option namespace: `programs.raycast.focus` (`raycast.nix:58`).

- `enable` (`raycast.nix:59`).
- `title` (str, default `Deep Work`) - Focus session title (`raycast.nix:61`).
- `filterMode` (enum `block`|`allow`, default `block`) - whether the blockable
  list is a blocklist or allowlist (`raycast.nix:68`).
- `duration.seconds` (positive int, default 900), `duration.title` (str,
  `15 minutes`), `duration.id` (opaque preset identifier) (`raycast.nix:77-94`).
- `categoryBlockableItems` / `blockableItems` (nullable str) - raw JSON for the
  blockable apps/sites lists; unmanaged by default because the value is a large
  Raycast-internal blob (`raycast.nix:96`, `:107`).

## What it produces

`config` (`raycast.nix:114`):

- **Assertion** (`raycast.nix:115-120`): the module is macOS-only; it asserts
  `pkgs.stdenv.hostPlatform.isDarwin` because it writes the `com.raycast.macos`
  defaults domain.
- **Activation script** `home.activation.raycastFocus` (`raycast.nix:122`),
  ordered after `writeBoundary`: writes the string keys
  (`raycast-startFocusSession-title`, `-filter-mode`) with `defaults write` and
  the data keys (`-duration`, optional blockable lists) with `defaults write
  -data` (hex via `od`).

Caveat from source (`raycast.nix:11-14`): Raycast rewrites these keys when you
edit a Focus session in its UI, so values apply at `home-manager switch` time
and a running Raycast may not pick them up until relaunched.

## How it is wired

Referenced directly as `homeModules.raycast` in `flake.nix:295`; import it into a
macOS home-manager configuration and set `programs.raycast.focus`. It pulls in no
extra packages (uses the macOS-bundled `/usr/bin/defaults`, `od`, `tr`).
