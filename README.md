<p align="center">
  <img src="assets/logo.svg" width="80" alt="ix" />
</p>

# Index

`index` builds ready-to-run [ix](https://ix.dev/) VM images from NixOS modules.
Every image targets AMD EPYC Gen 5 (`znver5`) and ships as an OCI archive.

Use it for runnable images and reusable service modules.

## Quick Check

```sh
nix build .#minecraft
nix run .#lint
```

The first image build is slow because the full closure compiles from source for
`znver5`. Later rebuilds reuse the local Nix store.

## What Is Here

- [`images/`](images/) contains runnable systems.
- [`modules/`](modules/) contains opt-in NixOS service modules.
- [`examples/`](examples/) contains standalone consumer fleets, including a
  daily Python scraper.
- [`packages/`](packages/) contains repo-owned tools such as
  [`llm-clippy`](packages/llm-clippy/).
- [`lib/`](lib/) contains the shared helper API used by the repo and consumers.

## Bad Fit If

You need generic x86_64 binaries, aarch64 images, or FreeBSD. This repo chooses
`-march=znver5` for the whole closure, so generic [nixpkgs](https://github.com/NixOS/nixpkgs)
cache hits are intentionally out of scope.

## Contributor Notes

See [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md) when you're ready to dig in.
