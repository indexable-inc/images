{
  pkgs,
  writeNushellApplication,
  package,
  dataDir,
  dropinDir,
  managedRoot,
  plugmanReloadEnabled,
  rconEnabled,
  ignoredPlugins,
  datapackWorlds,
  rconPort,
  rconPasswordFile,
  rconBroadcastToOps,
}:
let
  inherit (pkgs) lib;

  rootArgs = [
    "--data-dir"
    dataDir
    "--dropin-dir"
    dropinDir
    "--managed-root"
    managedRoot
  ];

  reloadArgs = lib.optional plugmanReloadEnabled "--plugman-reload";

  ignoredPluginArgs = lib.concatMap (plugin: [
    "--plugman-ignored-plugin"
    plugin
  ]) ignoredPlugins;

  datapackWorldArgs = lib.concatMap (world: [
    "--datapack-world"
    world
  ]) datapackWorlds;

  rconArgs = [
    "--rcon-port"
    (toString rconPort)
    "--rcon-password-file"
    rconPasswordFile
    "--rcon-broadcast-to-ops"
    (lib.boolToString rconBroadcastToOps)
  ]
  ++ lib.optional rconEnabled "--rcon-enable";

  args = rootArgs ++ reloadArgs ++ ignoredPluginArgs ++ datapackWorldArgs ++ rconArgs;
in
writeNushellApplication pkgs {
  name = "minecraft-sync-managed";
  text = ''
    # nu
    def main [] {
      exec ${lib.getExe package} ${lib.escapeShellArgs args}
    }
  '';
}
