# tonbo-artifacts

`packages/tonbo-artifacts` packages the
[Tonbo Artifacts](https://artifacts.tonbo.io/docs/overview/) CLI, a prebuilt
binary served from Tonbo's release host. It installs the binary as-is; there is
no build, patch, or wrapper.

## What this repo changes

`default.nix` is a minimal prebuilt-binary install
(`packages/tonbo-artifacts/default.nix`):

- Source: `fetchurl` of
  `https://artifacts.tonbo.dev/release/${version}/artifacts`, pinned by
  `version = "e16636b0e5ce"` (a release rev, not a semver tag) and an inline SRI
  `hash` (`packages/tonbo-artifacts/default.nix:6-16`).
- Build: `stdenvNoCC.mkDerivation` with `dontUnpack`/`dontBuild`; the install
  phase is one `install -Dm755 "$src" "$out/bin/artifacts"`
  (`packages/tonbo-artifacts/default.nix:18-28`). No ELF patching is applied.
- `meta`: `description = "Tonbo Artifacts CLI"`, `mainProgram = "artifacts"`,
  `platforms = [ "x86_64-linux" ]`
  (`packages/tonbo-artifacts/default.nix:30-35`). No `license`.

The flake output name is `tonbo-artifacts` but the installed command is
`artifacts` (`mainProgram`), so `nix run .#tonbo-artifacts` invokes `artifacts`.

## Build and wiring

- Flake output: `package.nix` sets `packageSet = true` and
  `flake.systems = [ "x86_64-linux" ]`
  (`packages/tonbo-artifacts/package.nix:1-5`), so `nix run .#tonbo-artifacts`
  resolves only on x86_64-linux (the only platform the binary is published and
  tagged for). No overlay.
- Bump: edit `version` and refresh `hash` with `nix-prefetch-url`. There is no
  `manifest.json` and no `updateScript`; unlike [yc](../yc/overview.md) and
  [claude-code](../claude-code/overview.md) it does not track an upstream
  "latest" pointer, so a bump is a manual rev change.
