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

`index` is the open-source layer [ix](https://ix.dev) publishes on top of its
closed-source VM primitives: ready-to-run OCI images, reusable NixOS service
modules, agent and developer tooling, and the Nix library that builds them all.

Every image targets x86_64 Linux and ships as an OCI archive. The same outputs
are a library, so you can build your own images, import the helpers, or drive
the bundled agent tools from a Python MCP server.

## Quick Check

```sh
nix build .#minecraft   # realize one image closure
nix run .#lint          # nixfmt, statix, deadnix, ast-grep
nix flake show          # list every package, module, and check
```

The first image build may be slow while Nix realizes the image closure. Later
rebuilds reuse cached store paths from the local Nix store and configured
substituters.

## What Is Here

- [`images/`](images/) holds runnable NixOS systems packaged as OCI archives:
  Minecraft servers, development environments, and a remote desktop.
- [`modules/`](modules/) holds opt-in NixOS service modules, auto-discovered so
  a new directory needs no registry edit.
- [`packages/`](packages/) holds repo-owned tools, including the
  [`mcp`](packages/mcp/) Python agent server, the [`tui`](packages/tui/) PTY
  driver, [`semantic-search`](packages/semantic-search/), and
  [`llm-clippy`](packages/llm-clippy/).
- [`lib/`](lib/) holds the shared helper and builder API used by the repo and by
  consumers.
- [`examples/`](examples/) holds standalone consumer fleets, including a daily
  Python scraper.
- [`site/`](site/) holds the public update log that tracks operator-facing
  changes.

## Agent Tooling

The [`mcp`](packages/mcp/) package is a Python MCP server on a pinned
interpreter. Sessions `import tui` to spawn and drive PTY-backed processes with
full vt100 emulation, and `import semantic_search` for content-addressed code
search, with no install step. It is how an agent reaches the same primitives
this repo ships.

## Bad Fit If

You need aarch64 images, FreeBSD, or a sealed appliance with almost no operator
tooling. This repo is tuned for ix VM workflows on x86_64 Linux.

## Feedback

Bug reports and enhancement requests go to [GitHub Issues](https://github.com/indexable-inc/index/issues). Security reports follow [SECURITY.md](SECURITY.md). Code changes land through pull requests against the `main` branch; see [CONTRIBUTING.md](CONTRIBUTING.md) for local setup, coding standards, and commit conventions.

## Contributor Notes

See [AGENTS.md](AGENTS.md) and [CONTRIBUTING.md](CONTRIBUTING.md) when you're ready to dig in.
