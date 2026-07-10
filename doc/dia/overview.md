# dia

`packages/dia` packages [Dia](https://www.diabrowser.com), The Browser
Company's AI browser, by unpacking its signed, notarized macOS `.dmg` verbatim.
There is no patching: the bundle is installed byte-for-byte so the code
signature stays valid. The repo owns the version pin (a generated
`manifest.json`) and an updater that refreshes it.

## What this repo changes

`default.nix` is a prebuilt GUI-bundle install with a maintainer-facing updater
(`packages/dia/default.nix`):

- Source pin: `version` and `hash` come from `manifest.json`
  (`lib.importJSON ./manifest.json`, `default.nix:24-25`; currently
  `1.34.1-81705`). `version` is `<short>-<build>` matching the upstream filename
  `Dia-<version>.dmg`. The `.dmg` is `fetchurl`ed from
  `releases.diabrowser.com/release/Dia-${version}.dmg`
  (`default.nix:65-68`).
- Verbatim install: `undmg` extracts the `.app` into the build dir
  (`sourceRoot = "."`), and the install phase copies `Dia.app` into
  `$out/Applications/` and symlinks the bundle executable to `$out/bin/dia`
  (`default.nix:70-87`). `dontConfigure`, `dontBuild`, and crucially
  `dontFixup = true`: the bundle is signed and notarized, so any byte rewrite
  (strip, patch, the default fixup) would void the signature
  (`default.nix:74-78`).
- `meta`: `mainProgram = "dia"`, `platforms = [ "aarch64-darwin" ]`,
  `sourceProvenance = [ binaryNativeCode ]`. `meta.license` is omitted (not
  tagged `licenses.unfree`) so the no-`allowUnfree` flake set can still
  `nix run .#dia`; terms are The Browser Company's Dia license
  (`default.nix:93-103`).

## Updater (`updateScript`)

`passthru.updateScript` is a `writeNushellApplication` that refreshes
`manifest.json` to a Dia release (`default.nix:32-59`). It is bound only on the
flake package path (the overlay context passes `writeNushellApplication = null`,
so `pkgs.dia` carries no updateScript, `default.nix:7-11`, `89-91`).

- `nix run .#dia.updateScript` with no argument tracks the `Dia-latest.dmg`
  pointer, reading the resolved version out of the `Content-Disposition`
  filename the CDN returns (there is no appcast/manifest); pass a version to pin
  an exact one (`default.nix:43-52`).
- It runs `nix store prefetch-file` to download the `.dmg` once and emit the SRI
  hash, then writes `{ version, hash }` to `packages/dia/manifest.json`
  (`default.nix:53-56`).
- Pinning by raw bytes makes any exact version installable and verifiable,
  unlike Homebrew which can only freeze an already-installed cask
  (`default.nix:17-20`).
- Run from the repo root; joins `nix run .#update` via `updateScript = true`
  (`packages/dia/package.nix:15`).

## Build and wiring

- Flake output: `nix run .#dia` / `nix build .#dia`, gated to aarch64-darwin.
  `package.nix` sets `packageSet`, `flake`, and `overlay` all to
  `systems = [ "aarch64-darwin" ]`, plus `updateScript = true`
  (`packages/dia/package.nix:6-15`).
- Platform constraint: Dia ships a single Apple-Silicon `.app` and requires
  macOS 14+ on M1+, so every target is gated to aarch64-darwin and the
  linux flake/overlay/update paths never see it
  (`packages/dia/package.nix:3-5`).
