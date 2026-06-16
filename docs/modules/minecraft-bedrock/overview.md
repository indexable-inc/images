# minecraft-bedrock

`modules/services/minecraft-bedrock/default.nix` runs the Minecraft Bedrock
Dedicated Server. Bedrock is a native Linux server, so it is deliberately
separate from the Java [minecraft](../minecraft/overview.md) loader family.

Option namespace: `services.minecraft-bedrock` (`default.nix:104`).

## What it runs

The module builds the server itself (`default.nix:22-65`): a
`stdenv.mkDerivation` that fetches `bedrock-server-1.26.14.1.zip` from
`minecraft.net` (pinned hash, `--http1.1` + a browser UA), autopatchelfs the
ELF, and installs `bedrock_server` with `meta.mainProgram`. This is the default
`package`.

## Public surface (options)

- `enable` (`default.nix:105`).
- `package` (default the built Bedrock server) (`default.nix:107`).
- `port` (port, default 19132) - IPv4 UDP (`default.nix:113`).
- `portv6` (port, default 19133) - IPv6 UDP (`default.nix:119`).
- `openFirewall` (bool, default true) - opens both UDP ports (`default.nix:125`).
- `settings` (keyValue) - `server.properties` values (`default.nix:131`).
- `allowlist` (json) - `allowlist.json` (`default.nix:137`).
- `permissions` (json) - `permissions.json` (`default.nix:143`).

## What it produces

- **Port claims** (`default.nix:151`): `minecraft-bedrock-ipv4` (udp, address
  `0.0.0.0`) and `minecraft-bedrock-ipv6` (udp, address `::`).
- **Settings defaults** (`default.nix:167`): `server-port`/`server-portv6`
  default to the typed ports, `enable-lan-visibility = false`, all `mkDefault`.
- **Firewall** opens both UDP ports when `openFirewall` (`default.nix:173`).
- **systemd.services.minecraft-bedrock** (`default.nix:178`):
  `ix.systemdHardening` + `WorkingDirectory=/var/lib/minecraft-bedrock`,
  `ExecStart` the server, `StateDirectory=minecraft-bedrock`,
  `KillSignal=SIGINT`, `TimeoutStopSec=30`. `preStart` (`default.nix:192`)
  symlinks the static server assets (behavior_packs, resource_packs, config,
  data, definitions, ...) from the package into the data dir and installs the
  generated `server.properties`, `allowlist.json`, `permissions.json`.

No health check is declared.

## How it is wired

Auto-discovered as `services/minecraft-bedrock`. The server package is built in
the module itself via `fetchurl` (no flake output, no nixpkgs package).
