# Minecraft server runtime.
#
# Loader-agnostic. Provides systemd unit, mods, Java runtime, port.
# `serverJar` and `dropinDir` are slots filled by a loader module (fabric,
# folia, neoforge, paper, purpur, spigot, sponge, vanilla) via module merging.
# `dropinDir` is where mod jars get symlinked: fabric/neoforge/sponge use
# `mods`, paper/folia/purpur/spigot use `plugins`.
#
# All server config files (server.properties, bukkit.yml, spigot.yml, NBT
# data, etc.) go through `serverFiles`. Mod config files go through
# `configFiles` (placed under config/).
{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.services.minecraft;

  dataDir = "/var/lib/minecraft";
  managedRoot = "/etc/minecraft";
  fileExt = path: lib.last (lib.splitString "." path);

  flattenProperties =
    value:
    let
      pairsFor =
        prefix: current:
        if builtins.isAttrs current && !lib.isDerivation current then
          lib.concatMap (name: pairsFor (prefix ++ [ name ]) current.${name}) (lib.attrNames current)
        else
          [
            {
              name = lib.concatStringsSep "." prefix;
              value = current;
            }
          ];
      pairs = pairsFor [ ] value;
      names = map (pair: pair.name) pairs;
      duplicateNames = lib.filter (
        name: builtins.length (lib.filter (candidate: candidate == name) names) > 1
      ) (lib.unique names);
    in
    assert lib.assertMsg (
      duplicateNames == [ ]
    ) "duplicate .properties keys after flattening: ${lib.concatStringsSep ", " duplicateNames}";
    lib.listToAttrs pairs;

  modCatalogType = types.submodule {
    options = {
      url = mkOption { type = types.str; };
      hash = mkOption {
        type = types.str;
        description = "SRI hash of the artifact at `url`. Used by `ix.artifacts.attachArtifactSources` to build the fetchurl derivation.";
      };
      src = mkOption {
        type = types.path;
        description = "Locked mod artifact.";
      };
      pluginName = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Runtime Bukkit plugin name, when it differs from the catalog slug.";
      };
    };
  };

  formatValueType = (pkgs.formats.json { }).type;

  modConfigType = types.submodule {
    freeformType = formatValueType;
  };

  pluginType = types.submodule {
    freeformType = formatValueType;

    options = {
      src = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = "Plugin jar. Leave unset to resolve the plugin from pluginCatalog by slug.";
      };
      pluginName = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Runtime Bukkit plugin name used by PlugManX reloads.";
      };
    };
  };

  worldType = types.submodule {
    options.generator = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Bukkit generator plugin name for this world.";
    };
  };

  playerType = types.submodule (
    { name, ... }:
    {
      options = {
        uuid = mkOption {
          type = types.str;
          example = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
          description = "Minecraft account UUID for this player.";
        };

        name = mkOption {
          type = types.str;
          default = name;
          defaultText = lib.literalMD "the attribute name under `services.minecraft.players`";
          description = "Minecraft player name written to access-control files.";
        };

        whitelist = mkOption {
          type = types.bool;
          default = false;
          description = "Whether to include this player in the generated whitelist.json.";
        };

        operator = {
          enable = mkOption {
            type = types.bool;
            default = false;
            description = "Whether to include this player in the generated ops.json.";
          };

          level = mkOption {
            type = types.ints.between 0 4;
            default = 4;
            description = "Minecraft operator permission level.";
          };

          bypassesPlayerLimit = mkOption {
            type = types.bool;
            default = false;
            description = "Whether this operator can join when the server is full.";
          };
        };
      };
    }
  );

  players = lib.attrValues cfg.players;
  playerUUIDs = map (player: player.uuid) players;
  duplicatePlayerUUIDs = lib.filter (
    uuid: builtins.length (lib.filter (candidate: candidate == uuid) playerUUIDs) > 1
  ) (lib.unique playerUUIDs);
  rawAccessFileNames = lib.intersectLists [
    "ops.json"
    "whitelist.json"
  ] (lib.attrNames cfg.serverFiles);

  bukkit = {
    worlds = lib.filterAttrs (_: world: world != { }) (
      lib.mapAttrs (
        _: world:
        lib.optionalAttrs (world.generator != null) {
          inherit (world) generator;
        }
      ) cfg.worlds
    );
  };

  whitelistEntries = map (player: {
    inherit (player) uuid name;
  }) (lib.filter (player: player.whitelist) players);

  operatorEntries = map (player: {
    inherit (player) uuid name;
    inherit (player.operator) level bypassesPlayerLimit;
  }) (lib.filter (player: player.operator.enable) players);

  accessFiles = {
    "whitelist.json" = whitelistEntries;
    "ops.json" = operatorEntries;
  };

  modJars = lib.mapAttrsToList (
    slug: _:
    let
      entry = cfg.modCatalog.${slug} or (throw "mod '${slug}' not in modCatalog");
      pluginName =
        cfg.autoReload.plugman.pluginNames.${slug}
          or (if entry.pluginName == null then slug else entry.pluginName);
    in
    {
      name = "${slug}.jar";
      path = entry.src;
      inherit pluginName;
    }
  ) cfg.mods;

  pluginJars = lib.mapAttrsToList (
    slug: plugin:
    let
      entry =
        if plugin.src == null then
          cfg.pluginCatalog.${slug} or (throw "plugin '${slug}' not in pluginCatalog")
        else
          plugin;
      pluginName =
        cfg.autoReload.plugman.pluginNames.${slug}
          or (if entry.pluginName == null then slug else entry.pluginName);
    in
    {
      name = "${slug}.jar";
      path = entry.src;
      inherit pluginName;
    }
  ) cfg.plugins;

  loaderEnabled = lib.genAttrs [
    "fabric"
    "folia"
    "paper"
    "purpur"
    "spigot"
    "sponge"
  ] (name: cfg.${name}.enable);

  bukkitLoaderEnabled = lib.any (name: loaderEnabled.${name}) [
    "folia"
    "paper"
    "purpur"
    "spigot"
  ];

  autoReloadDriver =
    if cfg.autoReload.driver != "auto" then
      cfg.autoReload.driver
    else if loaderEnabled.fabric then
      "jvm"
    else if bukkitLoaderEnabled then
      "plugman"
    else
      "none";

  autoReloadEnabled = cfg.autoReload.enable && autoReloadDriver != "none";
  jvmReloadEnabled = autoReloadEnabled && autoReloadDriver == "jvm";
  plugmanReloadEnabled = autoReloadEnabled && autoReloadDriver == "plugman";
  rconEnabled = cfg.rcon.enable || plugmanReloadEnabled;
  rconPort = if cfg.rcon.enable then cfg.rcon.port else cfg.autoReload.rconPort;
  rconPasswordFile =
    if cfg.rcon.enable then cfg.rcon.passwordFile else cfg.autoReload.rconPasswordFile;
  rconBroadcastToOps = if cfg.rcon.enable then cfg.rcon.broadcastToOps else false;
  java = lib.getExe' cfg.javaPackage "java";
  pluginConfigFiles = lib.optionalAttrs plugmanReloadEnabled {
    "plugins/PlugManX/config.yml" = {
      ignored-plugins = cfg.autoReload.plugman.ignoredPlugins;
      notify-on-broken-command-removal = true;
      auto-load = {
        enabled = false;
        check-every-seconds = 10;
      };
      auto-unload = {
        enabled = false;
        check-every-seconds = 10;
      };
      auto-reload = {
        enabled = false;
        check-every-seconds = 10;
      };
      showPaperWarning = true;
      version = 3;
    };
  };

  managedJars =
    modJars
    ++ pluginJars
    ++ lib.optionals plugmanReloadEnabled [
      {
        name = "PlugManX.jar";
        path = ix.artifacts.minecraft.plugins.plugmanx;
        pluginName = "PlugManX";
      }
    ];

  nbtFormats = {
    nbt = ix.mkMinecraftNbtFormat pkgs { format = "nbt"; };
    snbt = ix.mkMinecraftNbtFormat pkgs { format = "snbt"; };
    nbtGzip = ix.mkMinecraftNbtFormat pkgs {
      format = "nbt";
      flavor = "gzip";
    };
    nbtZlib = ix.mkMinecraftNbtFormat pkgs {
      format = "nbt";
      flavor = "zlib";
    };
  };

  # Infer serialization format from file extension.
  formatFor =
    path:
    let
      ext = fileExt path;
    in
    if lib.hasSuffix ".nbt.gz" path then
      nbtFormats.nbtGzip
    else if lib.hasSuffix ".nbt.zlib" path then
      nbtFormats.nbtZlib
    else
      {
        toml = pkgs.formats.toml { };
        json = pkgs.formats.json { };
        yaml = pkgs.formats.yaml { };
        yml = pkgs.formats.yaml { };
        properties = pkgs.formats.keyValue { };
        inherit (nbtFormats) nbt snbt;
      }
      .${ext} or (throw "minecraft managed files: unsupported extension .${ext} on '${path}'");

  normalizeFor = path: value: if fileExt path == "properties" then flattenProperties value else value;

  serverFiles = cfg.serverFiles // pluginConfigFiles;

  defaultWorldName = toString (cfg.properties."level-name" or "world");
  annotatedWorldNames = lib.unique ([ defaultWorldName ] ++ lib.attrNames cfg.worlds);
  mkXattrDefaults = kind: attributes: {
    attributes = lib.mapAttrs (_: lib.mkDefault) (
      {
        "user.ix.managed-by" = "nix";
        "user.ix.service" = "minecraft";
        "user.ix.kind" = kind;
      }
      // attributes
    );
  };
  mkCreatedXattrDefaults =
    kind: attributes:
    mkXattrDefaults kind attributes
    // {
      create = lib.mkDefault true;
    };
  regionDirectoriesFor = world: [
    {
      path = "${dataDir}/${world}/region";
      dimension = "overworld";
    }
    {
      path = "${dataDir}/${world}/DIM-1/region";
      dimension = "nether";
    }
    {
      path = "${dataDir}/${world}/DIM1/region";
      dimension = "end";
    }
  ];
  worldXattrs = lib.listToAttrs (
    lib.concatMap (
      world:
      [
        {
          name = "${dataDir}/${world}";
          value = mkCreatedXattrDefaults "minecraft.world" {
            "user.ix.minecraft.world" = world;
          };
        }
      ]
      ++ map (region: {
        name = region.path;
        value = mkCreatedXattrDefaults "minecraft.region-directory" {
          "user.ix.minecraft.world" = world;
          "user.ix.minecraft.dimension" = region.dimension;
        };
      }) (regionDirectoriesFor world)
    ) annotatedWorldNames
  );

  mkManaged =
    label: source:
    pkgs.runCommand "minecraft-managed-${label}" { } ''
      mkdir -p "$out"
      ${lib.concatStringsSep "\n" (
        lib.mapAttrsToList (
          path: value:
          let
            file = (formatFor path).generate (baseNameOf path) (normalizeFor path value);
          in
          "mkdir -p $out/${dirOf path}\nln -sf ${file} $out/${path}"
        ) source
      )}
    '';

  managed =
    let
      dropins = pkgs.runCommand "minecraft-managed-${cfg.dropinDir}" { } (
        ''
          mkdir -p "$out"
        ''
        + lib.concatMapStringsSep "\n" (jar: ''
          ln -s ${jar.path} "$out/${jar.name}"
          printf '%s\n' ${lib.escapeShellArg jar.pluginName} > "$out/${jar.name}.plugin-name"
        '') managedJars
      );
      configFiles = mkManaged "config" cfg.configFiles;
      serverRootFiles = mkManaged "server-files" serverFiles;
      access = mkManaged "access" accessFiles;
    in
    {
      inherit dropins;
      config = configFiles;
      serverFiles = serverRootFiles;
      inherit access;
      reloadRoots = [
        dropins
        configFiles
        serverRootFiles
      ];
      restartRoots = [ access ];
    };

  syncManaged = ix.mkMinecraftSyncManaged {
    inherit
      pkgs
      dataDir
      managedRoot
      plugmanReloadEnabled
      rconEnabled
      rconPort
      rconPasswordFile
      rconBroadcastToOps
      ;
    inherit (cfg) dropinDir;
    inherit (cfg.autoReload.plugman) ignoredPlugins;
  };

  reloadCommand = ix.writeNushellApplication pkgs {
    name = "minecraft-reload";
    runtimeInputs = [
      pkgs.minecraft-rcon
      syncManaged
    ];
    text = ''
      const driver = ${builtins.toJSON autoReloadDriver}
      const socket = ${builtins.toJSON cfg.autoReload.socketPath}
      const plan = ${builtins.toJSON "${dataDir}/.ix-managed-${cfg.dropinDir}.reload-plan"}

      def main [] {
        minecraft-sync-managed

        match $driver {
          "jvm" => {
            if not (($socket | path type) == "socket") {
              print --stderr $"minecraft hot reload socket is not ready at ($socket); synced managed files only"
              return
            }

            exec ${java} -cp ${pkgs.minecraft-hot-reload-agent}/share/minecraft-hot-reload-agent/minecraft-hot-reload-agent.jar dev.ix.minecraft.hotreload.HotReloadAgent $socket redefine-dir ${managedRoot}/managed-dropins
          }
          "plugman" => {
            if not ($plan | path exists) or ((open --raw $plan | str trim | is-empty)) {
              return
            }

            mut failed = false
            for row in (open --raw $plan | lines | parse "{action} {plugin}") {
              if (do --ignore-errors {
                minecraft-rcon --host 127.0.0.1 --port ${toString rconPort} --password-file ${builtins.toJSON rconPasswordFile} plugman $row.action $row.plugin
              }) == null {
                $failed = true
              }
            }

            if $failed {
              exit 1
            }
          }
          "none" => {}
          _ => {
            print --stderr $"unsupported minecraft auto reload driver: ($driver)"
            exit 1
          }
        }
      }
    '';
  };

  autoReloadJvmFlags = lib.optionals jvmReloadEnabled [
    "-javaagent:${pkgs.minecraft-hot-reload-agent}/share/minecraft-hot-reload-agent/minecraft-hot-reload-agent.jar=socket=${cfg.autoReload.socketPath}"
  ];

  javaArgs = [
    java
    "-XX:MaxRAMPercentage=${toString cfg.maxRAMPercentage}"
  ]
  ++ cfg.jvmFlags
  ++ autoReloadJvmFlags
  ++ [
    "-jar"
    "${cfg.serverJar}"
    "nogui"
  ];
in
{
  options.services.minecraft = {
    enable = mkEnableOption "Minecraft server runtime";

    version = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "1.21.11";
      description = ''
        Minecraft game version. Single source of truth for the server jar
        and mod catalog: loader modules derive `src` from
        `ix.artifacts.minecraft.servers."''${version}-''${loader}"` and the
        default `modCatalog` is built from
        `ix.artifacts.minecraft.modCatalogs.''${version}` (plus the
        cross-version `common` catalog).
      '';
    };

    serverJar = mkOption {
      type = types.package;
      description = "Server jar to launch. Set by a loader module (fabric/paper/vanilla).";
    };

    dropinDir = mkOption {
      type = types.str;
      default = "mods";
      description = "Subdirectory under the data dir where mod jars are symlinked. Loaders set this: fabric uses mods, paper uses plugins.";
    };

    maxRAMPercentage = mkOption {
      type = types.int;
      default = 85;
      description = "Max heap as a percentage of available system RAM. The JVM auto-scales to the VM's memory.";
    };

    mods = mkOption {
      type = types.attrsOf modConfigType;
      default = { };
      description = "Mods to install, keyed by Modrinth slug. Empty {} includes the jar with defaults. Attrsets with fields configure the mod (mod modules read these and generate config files).";
    };

    plugins = mkOption {
      type = types.attrsOf pluginType;
      default = { };
      description = "Bukkit-family plugins to install. Empty {} resolves a pinned catalog plugin by slug; attrsets with src install a local or private plugin jar.";
    };

    modCatalog = mkOption {
      type = types.attrsOf modCatalogType;
      default =
        let
          catalogs = ix.artifacts.minecraft.modCatalogs;
        in
        (catalogs.common or { })
        // (lib.optionalAttrs (cfg.version != null) (catalogs.${cfg.version} or { }));
      defaultText = lib.literalMD ''
        `ix.artifacts.minecraft.modCatalogs.common` merged with
        `ix.artifacts.minecraft.modCatalogs.''${version}` when
        `services.minecraft.version` is set.
      '';
      description = "Slug to locked mod artifact mapping. Defaults from `services.minecraft.version`; override per-key to add private or unpinned mods.";
    };

    pluginCatalog = mkOption {
      type = types.attrsOf modCatalogType;
      default = { };
      description = "Slug to locked Bukkit plugin artifact mapping.";
    };

    players = mkOption {
      type = types.attrsOf playerType;
      default = { };
      description = "Minecraft players keyed by a stable local name. Entries generate whitelist.json and ops.json by UUID, while preserving manual runtime additions during sync.";
    };

    whitelist = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to write white-list=true in server.properties.";
      };

      enforce = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to write enforce-whitelist=true, so online players are disconnected when removed from the whitelist.";
      };
    };

    javaPackage = mkOption {
      type = types.package;
      default = pkgs.temurin-jre-bin-25;
    };

    jvmFlags = mkOption {
      type = types.listOf types.str;
      default = [
        # Aikar's flags: https://mcflags.emc.gs
        "-XX:+UnlockExperimentalVMOptions"
        "-XX:+UseG1GC"
        "-XX:+ParallelRefProcEnabled"
        "-XX:MaxGCPauseMillis=200"
        "-XX:+DisableExplicitGC" # prevent plugins from triggering full GC

        # large young gen: MC allocates heavily per tick, then discards
        "-XX:G1NewSizePercent=30"
        "-XX:G1MaxNewSizePercent=40"
        "-XX:G1HeapRegionSize=8M" # fewer regions = less bookkeeping
        "-XX:G1ReservePercent=20" # headroom so promotion doesn't force emergency collection

        # mixed GC tuning: reclaim old-gen without long pauses
        "-XX:G1MixedGCCountTarget=4"
        "-XX:InitiatingHeapOccupancyPercent=15" # start concurrent mark early
        "-XX:G1MixedGCLiveThresholdPercent=90"
        "-XX:G1RSetUpdatingPauseTimePercent=5"

        "-XX:SurvivorRatio=32" # tiny survivor spaces: most objects die in eden
        "-XX:+PerfDisableSharedMem" # avoid mmap that causes GC stalls on some filesystems
        "-XX:MaxTenuringThreshold=1" # promote survivors immediately, don't copy between survivor spaces

        "-Dusing.aikars.flags=https://mcflags.emc.gs"
        "-Daikars.new.flags=true"
      ];
      description = "JVM flags used after heap sizing and before -jar.";
    };

    rcon = {
      enable = mkEnableOption "Minecraft RCON";

      port = mkOption {
        type = types.port;
        default = 25575;
        description = "TCP port for Minecraft RCON.";
      };

      passwordFile = mkOption {
        type = types.str;
        default = "${dataDir}/.ix-rcon-password";
        description = "State-local RCON password file. Generated on first start when absent.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to open the RCON port in the firewall.";
      };

      broadcastToOps = mkOption {
        type = types.bool;
        default = false;
        description = "Whether RCON commands should be broadcast to operators.";
      };
    };

    autoReload = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Reload managed mods/plugins during NixOS switch without restarting the Minecraft service when the active loader has a reload driver.";
      };

      driver = mkOption {
        type = types.enum [
          "auto"
          "jvm"
          "plugman"
          "none"
        ];
        default = "auto";
        description = "Reload driver. auto uses JVM class redefinition for Fabric and PlugManX for Bukkit-family loaders.";
      };

      socketPath = mkOption {
        type = types.str;
        default = "/run/minecraft-hot-reload/socket";
        description = "Unix-domain socket used by the JVM class redefinition agent.";
      };

      rconPort = mkOption {
        type = types.port;
        default = 25575;
        description = "Local RCON port used to ask PlugManX to reload Bukkit-family plugins.";
      };

      rconPasswordFile = mkOption {
        type = types.str;
        default = "${dataDir}/.ix-rcon-password";
        description = "State-local RCON password file used by the PlugManX reload command. Generated on first start when absent.";
      };

      plugman = {
        ignoredPlugins = mkOption {
          type = types.listOf types.str;
          default = [
            "PlugMan"
            "PlugManX"
            "PlugManBungee"
            "ViaVersion"
            "ViaBackwards"
            "ViaRewind"
            "ProtocolSupport"
            "ProtocolLib"
          ];
          description = "Plugins PlugManX should never manage during enable, disable, restart, load, reload, or unload operations.";
        };

        pluginNames = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "Managed plugin slug to Bukkit plugin name mapping for PlugManX commands when the jar slug differs from the runtime plugin name.";
        };
      };
    };

    configFiles = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Config files to place under config/. Keys are relative paths (format inferred from extension: .toml, .json, .yaml, .yml, .properties, .snbt, .nbt, .nbt.gz, .nbt.zlib). Values are Nix attrsets.";
    };

    properties = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Settings written to server.properties. Nested attrsets flatten to dotted properties keys.";
    };

    bukkit = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Settings written to bukkit.yml.";
    };

    worlds = mkOption {
      type = types.attrsOf worldType;
      default = { };
      description = "Bukkit worlds keyed by world name. Generator settings are rendered to bukkit.yml.";
    };

    serverFiles = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Files to place relative to the server root. Keys are paths and format is inferred from extension. Prefer services.minecraft.properties for server.properties, services.minecraft.bukkit for bukkit.yml, and services.minecraft.players for whitelist.json and ops.json so ix can reconcile Minecraft's mutable access files.";
    };

    port = mkOption {
      type = types.port;
      default = 25565;
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = duplicatePlayerUUIDs == [ ];
        message = "services.minecraft.players contains duplicate UUIDs: ${lib.concatStringsSep ", " duplicatePlayerUUIDs}";
      }
      {
        assertion = rawAccessFileNames == [ ];
        message = "services.minecraft.serverFiles cannot manage ${lib.concatStringsSep ", " rawAccessFileNames}; use services.minecraft.players so ix can reconcile Minecraft's mutable access files by UUID.";
      }
    ];

    services.minecraft = {
      properties = lib.mkMerge [
        {
          server-port = lib.mkDefault cfg.port;
          max-players = lib.mkDefault 100000;
          online-mode = lib.mkDefault true;
          enforce-secure-profile = lib.mkDefault true;
          gamemode = lib.mkDefault "survival";
          force-gamemode = lib.mkDefault false;
          pvp = lib.mkDefault true;
          hardcore = lib.mkDefault false;
          spawn-protection = lib.mkDefault 16;
          view-distance = lib.mkDefault 32;
          simulation-distance = lib.mkDefault 32;
          allow-flight = lib.mkDefault false;
          enable-command-block = lib.mkDefault false;
        }
        (lib.mkIf cfg.whitelist.enable {
          white-list = lib.mkDefault true;
          enforce-whitelist = lib.mkDefault cfg.whitelist.enforce;
        })
      ];

      bukkit = lib.mkIf (bukkit.worlds != { }) {
        inherit (bukkit) worlds;
      };

      serverFiles = lib.mkMerge [
        {
          "server.properties" = cfg.properties;
        }
        (lib.mkIf (cfg.bukkit != { }) {
          "bukkit.yml" = cfg.bukkit;
        })
      ];

    };

    ix.extendedAttributes = lib.mkMerge [
      {
        ${dataDir} = mkCreatedXattrDefaults "minecraft.server-root" { };
        "${dataDir}/${cfg.dropinDir}" = mkCreatedXattrDefaults "minecraft.dropins" {
          "user.ix.minecraft.dropin-dir" = cfg.dropinDir;
        };
        "${dataDir}/config" = mkCreatedXattrDefaults "minecraft.config" { };
      }
      worldXattrs
    ];

    networking.firewall.allowedTCPPorts = [
      cfg.port
    ]
    ++ lib.optionals cfg.rcon.openFirewall [ rconPort ];
    environment.etc = {
      "minecraft/managed-dropins".source = managed.dropins;
      "minecraft/managed-config".source = managed.config;
      "minecraft/managed-server-files".source = managed.serverFiles;
      "minecraft/managed-access".source = managed.access;
    };

    systemd.services.minecraft = {
      description = "Minecraft server";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      reloadTriggers = lib.optionals autoReloadEnabled managed.reloadRoots;
      restartTriggers = lib.optionals (!autoReloadEnabled) managed.reloadRoots ++ managed.restartRoots;
      serviceConfig =
        ix.systemdHardening
        // {
          Type = "simple";
          WorkingDirectory = dataDir;
          ExecStart = lib.escapeShellArgs javaArgs;
          ExecReload = lib.getExe reloadCommand;
          Restart = "on-failure";
          StateDirectory = "minecraft";
        }
        // lib.optionalAttrs jvmReloadEnabled {
          RuntimeDirectory = "minecraft-hot-reload";
        };
      preStart = ''
        mkdir -p ${dataDir}/${cfg.dropinDir}
        echo "eula=true" > ${dataDir}/eula.txt
        ${lib.getExe syncManaged}
      '';
    };
  };
}
