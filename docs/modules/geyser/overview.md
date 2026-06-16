# geyser

`modules/services/geyser/default.nix` installs Geyser, the Bedrock-to-Java
protocol bridge, as a Velocity proxy plugin. It lets Bedrock (mobile/console)
clients join a Java server fronted by [velocity](../velocity/overview.md). It
does not run its own systemd unit: it composes onto `services.velocity`.

Option namespace: `services.geyser` (`default.nix:77`).

## What it runs

The Geyser Velocity plugin jar. Default `package` is
`ix.artifacts.minecraft.velocityPluginCatalog.geyser-velocity.src`
(`default.nix:86`). `platform` is fixed to `velocity` (`default.nix:80`).

## Public surface (selected options)

Geyser exposes the full Geyser `config.yml` as typed options merged with a raw
`settings` escape hatch (`default.nix:377`). Highlights:

- `enable`, `platform`, `package` (`default.nix:78-91`).
- `bedrock.*`: `address` (`0.0.0.0`), `port` (udp, default 19132),
  `openFirewall` (true), `cloneRemotePort`, `motd1`/`motd2`, `serverName`,
  `compressionLevel`, `enableProxyProtocol` (`default.nix:93-147`).
- `remote.*`: `address` (`auto`), `port` (25565), `authType`
  (`online`|`offline`|`floodgate`, default `online`), `useProxyProtocol`,
  `forwardHostname` (`default.nix:149-183`).
- `floodgateKeyFile`, `commandSuggestions`, `passthrough.*`, `maxPlayers` (100),
  `debug`, skull/render/item options, `metrics.enable`, `mtu` (1400),
  `useDirectConnection`, `disableCompression` (`default.nix:185-375`).

## What it produces

- **Composes onto Velocity** (`default.nix:384-392`): sets
  `services.velocity.enable = mkDefault true`, adds
  `services.velocity.plugins.geyser = { src = package; fileName =
  "Geyser-Velocity.jar"; }`, and writes the rendered `config.yml` to
  `services.velocity.configFiles."plugins/geyser/config.yml"`. The rendered config
  is the typed options mapped to Geyser's hyphenated keys plus `config-version =
  4`, merged with `settings` (`default.nix:21-74`).
- **Port claim + firewall** (`default.nix:394-400`):
  `ix.networking.portClaims.geyser` (udp, `bedrock.address`/`bedrock.port`) and
  `networking.firewall.allowedUDPPorts` gated by `bedrock.openFirewall`.

## Relationship to Floodgate

When [floodgate](../floodgate/overview.md) is also enabled, it defaults
`services.geyser.remote.authType` to `floodgate`
(`floodgate/default.nix:196-198`), so Bedrock players authenticate through the
Floodgate identity bridge rather than needing a Java account.

## How it is wired

Auto-discovered as `services/geyser`. Plugin jar comes from
`ix.artifacts.minecraft.velocityPluginCatalog`. Runs only as a managed plugin of
the [velocity](../velocity/overview.md) unit.
