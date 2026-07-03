# Paper server jar. https://papermc.io
# Server jar comes from `ix.artifacts.minecraft.servers."${version}-paper"`,
# which aliases to `ix.artifacts.minecraft.paperServers.${version}.src`.
{
  ix,
  config,
  lib,
  ...
}:
ix.mkMinecraftLoader {
  inherit ix config lib;
  name = "paper";
  dropinDir = "plugins";
  configFragment = _: let
    cfg = config.services.minecraft;
    versionCatalogs = ix.artifacts.minecraft.paperPluginCatalogs;
    velocityCfg = config.services.velocity;
    # Paper behind Velocity needs paper-global.yml proxies.velocity to
    # mirror the shared forwarding secret. Default the Paper-side block
    # from `services.velocity.forwarding.secret` so a preset writes the
    # secret once. `forwarding.secretFile` runtime secrets cannot be
    # propagated at eval time, so callers using them still set the
    # paper-global block themselves.
    hasVelocityForwarding = velocityCfg.enable && velocityCfg.forwarding.secret != null;
  in {
    services.minecraft = {
      pluginCatalog =
        if cfg.version != null && builtins.hasAttr cfg.version versionCatalogs
        then versionCatalogs.${cfg.version}
        else ix.artifacts.minecraft.paperPluginCatalog;

      configFiles = lib.optionalAttrs hasVelocityForwarding {
        "paper-global.yml".proxies.velocity = {
          enabled = lib.mkDefault true;
          secret = lib.mkDefault velocityCfg.forwarding.secret;
          online-mode = lib.mkDefault true;
        };
      };
    };
  };
}
