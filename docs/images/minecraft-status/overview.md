# minecraft-status

`images/games/minecraft-status` is a minimal Fabric Minecraft server tuned to act
as the ix status / lifecycle canary: small, fast to boot, and reachable through
ix port-forwarding rather than a guest-managed public port. Flake output
`.#minecraft-status`. It is a hand-rolled single-version image (no `versions.nix`),
separate from the multi-variant [minecraft](../minecraft/overview.md) image.

## What it builds

`images/games/minecraft-status/default.nix` (30 lines):

- `ix.image = { name = "minecraft-status"; tag = "1.21.11-fabric"; }` (`:2-5`).
- turns the NixOS firewall unit OFF entirely
  (`networking.firewall.enable = false`, `:10`): the canary is reached over ix
  port-forwarding and exposes no north-south port, so dropping the firewall unit
  removes boot work from the five-minute lifecycle probe (`:7-9`).
- enables a Fabric server at 1.21.11 with the firewall closed and a stripped-down
  property set sized for the canary (`:12-29`):

```nix
services.minecraft = {
  enable = true;
  version = "1.21.11";
  fabric.enable = true;
  openFirewall = false;
  properties = {
    motd = "ix status Minecraft";
    max-players = 8; online-mode = false;
    enforce-secure-profile = false; spawn-protection = 0;
    view-distance = 6; simulation-distance = 4;   # small, the canary loads spawn + 6 bot logins
  };
};
```

## Composed module

Same `services.minecraft` runtime as the main [minecraft](../minecraft/overview.md)
image (`modules/services/minecraft/default.nix`), here with the Fabric loader and
`openFirewall = false`. With the firewall closed, the relevant health check is
`minecraft-status`, which probes the listener on `127.0.0.1:<port>` inside the
guest via `ix.packages.mc-probe` and asserts the MOTD substring
(`modules/services/minecraft/default.nix:1206-1225`). Because the loopback probe
runs in-guest, it works even though no public port is opened. `online-mode =
false` and `enforce-secure-profile = false` let the canary's bot logins connect
without Mojang auth.

## Build

```
nix build .#minecraft-status
```

## Notes

- The low `view-distance`/`simulation-distance` and `max-players = 8` keep the
  five-minute lifecycle probe cheap; this image is intentionally not a general
  play server. Use [minecraft](../minecraft/overview.md) for that.
- No dedicated image eval-test group is attached to this name; the
  `services.minecraft` module assertions and the SLP health check are its gate.
  The `minecraft-status` health-check command shape is itself pinned by
  module-level tests (`tests/default.nix:2683-2686,2802-2804`).
