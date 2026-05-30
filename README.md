<p align="center">
  <img src="assets/logo.svg" width="80" alt="index" />
</p>

<p align="center">
  <a href="https://antithesis.com/"><img src="https://img.shields.io/badge/Antithesis-tested-00B214?labelColor=7F39DA&style=flat" alt="Antithesis tested" /></a>
  <!-- OpenSSF Scorecard badge hidden until the rolling Code-Review score
       and CII Best Practices badge catch up; surface it once both move. -->
  <!-- <a href="https://scorecard.dev/viewer/?uri=github.com/indexable-inc/index"><img src="https://api.scorecard.dev/projects/github.com/indexable-inc/index/badge" alt="OpenSSF Scorecard" /></a> -->
</p>

<p align="center">
  <a href="https://ix.dev">ix.dev</a>
</p>

# Index

`index` builds ready-to-run VM images from NixOS modules. Every image targets
x86_64 Linux and ships as an OCI archive. It is the open-source layer over the
[ix](https://ix.dev) sandbox platform.

Use it for runnable images and reusable service modules.

## Quick Check

```sh
nix build .#minecraft
nix run .#lint
```

The first image build may be slow while Nix realizes the image closure. Later
rebuilds reuse cached store paths from the local Nix store and configured
substituters.

## What Is Here

- [`images/`](images/) contains runnable systems.
- [`modules/`](modules/) contains opt-in NixOS service modules.
- [`examples/`](examples/) contains standalone consumer fleets, including a
  daily Python scraper.
- [`packages/`](packages/) contains repo-owned tools such as
  [`llm-clippy`](packages/llm-clippy/).
- [`lib/`](lib/) contains the shared helper API used by the repo and consumers.

## Bad Fit If

You need aarch64 images, FreeBSD, or a sealed appliance with almost no operator
tooling. This repo is tuned for ix VM workflows on x86_64 Linux.

## Feedback

Bug reports and enhancement requests go to [GitHub Issues](https://github.com/indexable-inc/index/issues). Security reports follow [SECURITY.md](SECURITY.md). Code changes land through pull requests against the `main` branch; see [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, coding standards, and commit conventions.

## Contributor Notes

See [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md) when you're ready to dig in.
