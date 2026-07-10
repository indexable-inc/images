# drgn

`packages/drgn` is the index repo's Nix repackaging of
[drgn](https://github.com/osandov/drgn), Meta's programmable debugger for live
processes and kernels. It is a thin packaging/wrapper unit: it builds drgn from a
pinned upstream release and exposes it as a flake output and an overlay
attribute. It does not modify drgn's behavior; this page describes what is
packaged and how it is wired, not drgn's own API.

drgn complements `pahole`'s type-layout queries with live struct-graph
traversal: starting from a typed root, it dereferences pointers, walks intrusive
lists, and dumps fields off a running process or kernel over `/proc/kcore`
(`default.nix:42-48`). It is included in the base system profile so any switched
ix system has it (`modules/profiles/base/default.nix:521-527`).

## What is packaged

- `python3.pkgs.buildPythonApplication` (`pyproject = true`), `pname = "drgn"`,
  `version = "0.2.0"` (`default.nix:14-17`).
- Source is `ix.drgnSrc` (`default.nix:19`), a non-flake input pinned in
  `flake.nix` to the upstream tag `v0.2.0` with submodules
  (`flake.nix:95-97`: `git+https://github.com/osandov/drgn?ref=refs/tags/v0.2.0&submodules=1`),
  exposed to packages as `drgnSrc` in `lib/default.nix:429`.
- `build-system = [ setuptools ]`. drgn's `setup.py` shells out to autotools
  (`autoreconf -i`, `./configure`, `make`) via `build_ext`, so the
  `autoconf`/`automake`/`libtool`/`pkg-config` quartet plus `gcc` and `gnumake`
  are `nativeBuildInputs` (`default.nix:23-37`).
- `buildInputs = [ elfutils ]`: libdrgn's optional features (libkdumpfile core
  dumps, libdebuginfod, lzma, pcre2, json-c) auto-detect and stay disabled when
  absent; the target workload (live struct-graph traversal over `/proc/kcore`)
  needs only `libelf` + `libdw`, both from elfutils (`default.nix:25-39`).
- `meta`: `mainProgram = "drgn"`, license LGPL-2.1+, platforms `x86_64-linux`
  and `aarch64-linux` (`default.nix:41-57`).

## Flake output and platform gating

- Flake output `drgn` on Linux: `nix run .#drgn`, `nix build .#drgn`.
- Also an overlay attribute (`overlay = true`, `package.nix:19`), so `pkgs.drgn`
  resolves; the base system profile consumes it that way.
- drgn debugs over `/proc/kcore`, so it builds only on Linux. The flake output
  and the darwin package-set attr are gated to `x86_64-linux` + `aarch64-linux`
  (`package.nix:11-18`) so `nix flake check` never forces a package nixpkgs
  refuses to evaluate off-platform. The overlay stays unconditional (an
  `overlay.systems` filter would force `hostPlatform.system` while building the
  overlay spine and infinite-loop); `pkgs.drgn` on darwin is lazy and only the
  Linux base profile ever forces it (`package.nix:3-14`).

## Why it is in this domain

drgn pairs with the rest of vm-fleet as the live-introspection tool for what runs
inside a system: where [vmkit](../vmkit/overview.md) boots and drives a guest,
drgn inspects a running process or kernel from the inside. The pin tracks the
v0.2.0 upstream release until the open nixpkgs PR lands and the pin moves
(`modules/profiles/base/default.nix:524-527`).
