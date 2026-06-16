# minecraft-bedrock

`images/games/minecraft-bedrock` builds a Minecraft Bedrock Dedicated Server.
Bedrock is a native Linux binary, so it is a separate module family from the Java
[minecraft](../minecraft/overview.md) loader stack. Flake output
`.#minecraft-bedrock`.

## What it builds

`images/games/minecraft-bedrock/default.nix` (18 lines):

- `ix.image.name = "minecraft-bedrock"` and a tag that tracks the pinned server
  version: `tag = config.services.minecraft-bedrock.package.version` (`:4-9`), so
  bumping the Bedrock package in the module moves the image tag automatically.
- enables the server with a couple of settings (`:11-17`):

```nix
services.minecraft-bedrock = {
  enable = true;
  settings = { server-name = "ix-powered Bedrock"; max-players = 20; };
};
```

## Composed module: `services.minecraft-bedrock`

Defined in `modules/services/minecraft-bedrock/default.nix`. It packages the
Bedrock zip itself (`bedrockServer`, version `1.26.14.1`,
`pkgs.fetchurl` + `autoPatchelfHook` for the native ELF, `:20-65`). Key surface:

- `enable` (`:105`), `package` (default the built `bedrockServer`, `:107-111`).
- `port` (IPv4 UDP, default 19132, `:113-117`), `portv6` (IPv6 UDP, default
  19133, `:119-123`), `openFirewall` (default true, `:125-129`).
- `settings` (`server.properties` key/value, `:131-135`), `allowlist`
  (`allowlist.json`, `:137-141`), `permissions` (`permissions.json`, `:143-147`).

Runtime wiring (`:150-199`): claims both UDP ports
(`minecraft-bedrock-ipv4` on `0.0.0.0`, `minecraft-bedrock-ipv6` on `::`),
seeds `server-port`/`server-portv6` and `enable-lan-visibility = false`, opens
the firewall for both UDP ports when `openFirewall`, and runs
`systemd.services.minecraft-bedrock` (hardened, `KillSignal = SIGINT`,
`StateDirectory = minecraft-bedrock`). `preStart` symlinks the static server
files out of the package into `/var/lib/minecraft-bedrock` and installs the
generated `server.properties`/`allowlist.json`/`permissions.json` (`:178-198`).

## Build

```
nix build .#minecraft-bedrock
```

Bedrock listens on UDP 19132 (IPv4) and UDP 19133 (IPv6); both are opened by
default. The base platform also opens its standard ix-console/ix-agent ports; see
[common](../common.md).

## Eval test (`tests/default.nix:3718+`)

The `minecraft-bedrock` test group is attached to this image name and pins the
Bedrock module wiring (ports, settings, service).

## Notes

- Bump the Bedrock version by editing `version` (and the `hash`) in
  `modules/services/minecraft-bedrock/default.nix:20-34`; the image tag follows
  automatically.
