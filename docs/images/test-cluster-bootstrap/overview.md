# test-cluster-bootstrap

`images/system/test-cluster-bootstrap` is the bare NixOS bootstrap image the
fleet uses to materialize a node that does not exist yet. It is the smallest
image in the tree: just a name, a tag, and a hostname, riding entirely on the
base platform. Flake output `.#test-cluster-bootstrap`.

## What it builds

`images/system/test-cluster-bootstrap/default.nix` (8 lines, takes no module
args):

```nix
ix.image = {
  name = "ix/test-cluster-bootstrap";
  tag = "zstd-tools-2026-05-12";          # default.nix:2-5
};
networking.hostName = "test-cluster-bootstrap";   # default.nix:7
```

It enables no services. Everything it carries comes from the auto-enabled base
profile and `platform.nix` (container boot, nftables firewall with ix-console
and ix-agent ports, Nushell, the operator toolchain); see [common](../common.md).
The tag is hand-bumped to capture closure changes (the `zstd`/tools state the
bootstrap depends on).

## How it is used

This is not a workload image; it is the seed image for fleet node creation. The
image library evaluates it once as the canonical bootstrap and reads its name/tag
so the fleet default and the published image cannot drift
(`lib/image/default.nix:99-105`):

```nix
bootstrapImage =
  (evalImageConfig {
    modules = [ (paths.images + "/system/test-cluster-bootstrap") ];
  }).ix.image;
```

A fleet switch that finds a missing node creates it from
`registry.ix.dev/${bootstrap.name}:${bootstrap.tag}`
(`tests/default.nix:4470-4480`, asserting
`fleetPlan.web.bootstrapImage == "registry.ix.dev/${bootstrap.name}:${bootstrap.tag}"`).
The full fleet/lifecycle machinery (`mkFleet`, `ix-fleet`) is owned by
[vm-fleet](../../vm-fleet/common.md).

## Build

```
nix build .#test-cluster-bootstrap
```

## Notes

- Because the bootstrap is intentionally minimal, any change to the base profile
  fans out into this image and every other discovered image; bump the tag when
  the closure meaningfully changes so consumers re-pull.
