# yc

`packages/yc` packages the Y Combinator CLI (`yc`: search Bookface and chat with
the YC Agent from the terminal), installing the upstream prebuilt per-platform
binaries. The repo owns a generated `manifest.json` pin and an updater. It is
the worked example, alongside [claude-code](../claude-code/overview.md), for the
prebuilt-binary intake shape, but with NO upstream provenance check.

## What this repo changes

`default.nix` selects the right prebuilt binary, installs it, and patches it for
NixOS on Linux (`packages/yc/default.nix`):

- Source pin: `version` and per-platform `{ slug, hash }` come from
  `manifest.json` (`lib.importJSON`, `default.nix:16-17`; currently `0.0.8` for
  four platforms). The binary for the host system is `fetchurl`ed from
  `${baseUrl}/${version}/yc-${target.slug}` where `target` is
  `manifest.platforms.${system}`, with a clear `throw` listing supported systems
  if the host is unsupported (`default.nix:19-29`).
- Install (`default.nix:87-91`): `install -Dm755 $src $out/bin/yc`. On Linux,
  `autoPatchelfHook` patches the dynamically-linked interpreter to the Nix store
  (`default.nix:83-85`); Darwin binaries need no patching.
- `meta`: `mainProgram = "yc"`, `platforms = builtins.attrNames
  manifest.platforms`, `sourceProvenance = [ binaryNativeCode ]`. `meta.license`
  is omitted (not `licenses.unfree`) so the no-`allowUnfree` flake set can
  `nix run .#yc`; terms are Y Combinator's (`default.nix:97-107`).

## Updater (`updateScript`) and the no-provenance caveat

`passthru.updateScript` is a `writeNushellApplication`, bound only on the flake
package path (the overlay passes `writeNushellApplication = null`, so `pkgs.yc`
carries no updateScript, `default.nix:7-11`, `93-95`):

- `nix run .#yc.updateScript -- [version]` tracks the upstream `cli/latest`
  pointer (a 6-byte text file holding the version) when no version is given,
  then `nix store prefetch-file`s all four platform binaries and writes
  `{ version, platforms }` with per-platform SRI hashes to
  `packages/yc/manifest.json` (`default.nix:41-76`). The S3 bucket denies
  `ListBucket`, hence the `latest` pointer rather than enumerating versions.
- No provenance check: unlike claude-code, Y Combinator publishes no signed
  manifest, so the updater pins whatever bytes the release host serves (the
  version tag is not even immutable; `0.0.5` was observed republished under the
  same tag). The `nix build .#yc` step in the update workflow only proves the
  pinned bytes fetch and patch, not that they are authentic, so the real gate is
  human review of the four hash changes in the auto-bump PR
  (`default.nix:31-40`).

## Build and wiring

- Flake output: `nix run .#yc` / `nix build .#yc`, plus `pkgs.yc` (overlay).
  `package.nix` sets `packageSet`, `flake`, `overlay`, and `updateScript` all
  `true` (`packages/yc/package.nix`).
- Joins `nix run .#update` via `updateScript = true`, which runs every flagged
  package's updater in parallel (`lib/per-system.nix:461-501`); the `update.yml`
  workflow runs it hourly into one PR.
- Platforms: aarch64/x86_64 darwin and linux (the four `manifest.json` keys).
