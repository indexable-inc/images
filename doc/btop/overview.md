# btop

`packages/btop` repackages [btop](https://github.com/aristocratos/btop), the
resource monitor (CPU, memory, disk, network, process TUI), rebuilt from a
repo-owned fork instead of the upstream source. It is the minimal repackage
shape: take the nixpkgs `btop` derivation and override only its source.

## What this repo changes

`default.nix` is the whole delta: `btop.overrideAttrs` swaps `src` to
`ix.btopSrc` and rewrites `meta.homepage` to the fork
(`packages/terminal/btop/default.nix:6-11`). Everything else (build inputs, phases,
flags, the rest of `meta`) is inherited from nixpkgs `btop` unchanged.

- Source pin: `ix.btopSrc` resolves to the `btop-src` flake input, a
  `flake = false` GitHub source pinned at a commit rev
  (`flake.nix:90-93`, `github:indexable-inc/btop/711f4a1...`), threaded into the
  `ix` bundle as `btopSrc` (`lib/default.nix:428`). The fork carries whatever
  repo-specific changes are committed at that rev; this package just builds it.
- `meta.homepage` points at `indexable-inc/btop` so the built package advertises
  the fork, not upstream (`packages/terminal/btop/default.nix:10`).

## Build and wiring

- Flake output: `nix run .#btop` / `nix build .#btop`. `package.nix` sets
  `packageSet = true` and `flake = true` (`packages/terminal/btop/package.nix:1-5`); no
  overlay, so `pkgs.btop` stays the plain nixpkgs monitor.
- Bump: `nix flake update btop-src` (or repoint the input rev in `flake.nix`).
  There is no `manifest.json` and no `updateScript`; the flake lock owns the
  pin.
- Platforms: inherited from nixpkgs `btop` (unix); no extra `systems` gate.

Because the derivation is `overrideAttrs` over the upstream package, a nixpkgs
bump to `btop` (new build inputs, phase changes) flows through automatically;
only a source incompatible with the upstream build recipe would need work here.
