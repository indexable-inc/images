# floodgate

`modules/services/floodgate/default.nix` installs Floodgate, the Bedrock
identity/auth bridge, as a Velocity plugin beside [geyser](../geyser/overview.md).
Floodgate lets Bedrock players join without a Java (Mojang/Microsoft) account.
Like Geyser it runs no unit of its own; it composes onto
[velocity](../velocity/overview.md).

Option namespace: `services.floodgate` (`default.nix:58`).

## What it runs

The Floodgate Velocity plugin jar. Default `package` is
`ix.artifacts.minecraft.velocityPluginCatalog.floodgate-velocity.src`
(`default.nix:67`). `platform` is fixed to `velocity` (`default.nix:61`).

## Public surface (selected options)

- `enable`, `platform`, `package` (`default.nix:59-72`).
- `keyFileName` (`key.pem`), `usernamePrefix` (`.`), `replaceSpaces` (true),
  `sendFloodgateData` (false) (`default.nix:74-96`).
- `disconnect.{invalidKey,invalidArgumentsLength}` - Geyser-user disconnect
  messages (`default.nix:98`).
- `playerLink.*`: `enable` (true), `requireLink`, `enableOwnLinking`, `allowed`,
  `linkCodeTimeout` (300s), `type` (`sqlite`), `enableGlobalLinking` (true)
  (`default.nix:112-154`).
- `metrics.{enable,uuid}` - bStats (`default.nix:156`).
- `settings` / `proxySettings` - raw `config.yml` / `proxy-config.yml` merged
  over the typed options (`default.nix:170`, `:176`).

## What it produces

The whole `config` (`default.nix:183`) composes onto other modules; it declares
no port claim:

- **Velocity plugin** (`default.nix:184-194`): sets `services.velocity.enable =
  mkDefault true`, adds `services.velocity.plugins.floodgate = { src = package;
  fileName = "floodgate-velocity.jar"; }`, and writes two managed config files,
  `plugins/floodgate/config.yml` (rendered from the typed options + `config-version
  = 3`, merged with `settings`) and `plugins/floodgate/proxy-config.yml` (from
  `sendFloodgateData` + `proxySettings`).
- **Geyser integration** (`default.nix:196-198`): when `services.geyser.enable`,
  defaults `services.geyser.remote.authType` to `floodgate` so Geyser hands
  Bedrock identities to Floodgate.

## How it is wired

Auto-discovered as `services/floodgate`. Plugin jar comes from
`ix.artifacts.minecraft.velocityPluginCatalog`. Runs only as a managed plugin of
the [velocity](../velocity/overview.md) unit, paired with
[geyser](../geyser/overview.md).
