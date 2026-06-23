# Survival Server

One ix fleet node running a [Paper](https://papermc.io/software/paper)
survival backend behind [Velocity](https://papermc.io/software/velocity), with
[Geyser](https://geysermc.org/) and
[Floodgate](https://geysermc.org/wiki/floodgate/) on the proxy so Java and
Bedrock clients share the same world.

## Run

```sh
# From the index repo root.
nix run .#minecraft-survival-up
```

## Shape

[`minecraft.nix`](minecraft.nix) wires four listeners and keeps the backend
Paper port private to the image firewall:

- Velocity accepts Java clients on TCP `25565`.
- Geyser accepts Bedrock clients on UDP `19132`.
- Paper listens on TCP `25566` for local proxy traffic.
- RCON stays local for PlugManX reloads.

Velocity modern forwarding is enabled in `velocity.toml` and in
`paper-global.yml`. The checked-in forwarding secret is an example value. Replace
it before real players join, especially if the backend port is reachable from
another VM or a manual firewall edit.

## Bad Fit If

Use separate fleet nodes when you want several public survival servers all on
the natural Java port. One image can host a proxy plus one backend cleanly; a
network wants topology once the backends become independently scaled worlds.
