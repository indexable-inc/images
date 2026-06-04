# Non-NixOS OCI images

These examples build OCI images on a non-Nix base (ubuntu, debian) with Nix
packages layered on top, using `index.lib.mkNonNixImage`. They cover the path a
plain-distro image takes, the one the all-NixOS `images/` tree does not exercise.

Build one:

```
nix build .#non-nix-ubuntu
nix build .#non-nix-debian
```

Each archive is a standard OCI image, so `ix image push <name> registry.ix.dev/...`
works exactly as it does for the NixOS images.

## Why the leading underscore

The directory is `_non-nix-oci` so the example fleet discovery skips it. The
entries under `examples/` are NixOS fleets driven by `mkFleet` (each exposes
`ix fleet up/health/...` wrappers). These images are not fleets and not NixOS
systems, so they are discovered separately and surfaced as image packages and
`image-non-nix-*` checks, the same validation path the `images/` tree uses.

## How it differs from `mkImage`

`mkImage` builds a NixOS system closure: systemd as PID 1, `/init` as the
entrypoint, the closure as the rootfs. `mkNonNixImage` keeps the base image's
own userland as the rootfs and adds Nix store paths as extra layers. There is no
systemd, no `/init`; the entrypoint is whatever the base or your `config`
provides.

## Reproducibility

The base is pulled by digest via `dockerTools.pullImage`, not by tag, so the
build does not depend on what `ubuntu:24.04` points at today. To bump a base:

```
nix run nixpkgs#nix-prefetch-docker -- --os linux --arch amd64 \
  --image-name ubuntu --image-tag 24.04
```

Copy the `imageDigest` and `hash` it prints into the example.
