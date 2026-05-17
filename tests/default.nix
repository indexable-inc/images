# Eval tests. Each image with image-specific assertions has its own group
# below, exposed as `imageTests.<name>` so it can be attached to the image
# derivation via `passthru.tests`. `eval` aggregates them along with the
# cross-image checks (fleet, helpers).
{
  nixpkgs,
  ix,
}:
let
  inherit (nixpkgs) lib;
  inherit (ix) pkgs;
  fs = lib.fileset;

  versions = import ../images/games/minecraft/versions.nix {
    inherit lib;
    inherit (ix) artifacts;
  };
  defaultMinecraftVersion = versions.default;
  defaultMinecraftModule = versions.${defaultMinecraftVersion};

  # Thin wrapper to keep call sites as plain lists; delegates to ix.evalImageConfig
  # so tests exercise the same evaluation path as production image builds.
  evalConfig = modules: ix.evalImageConfig { inherit modules; };

  minecraft =
    let
      config = evalConfig [
        ../images/games/minecraft
        defaultMinecraftModule
      ];
    in
    {
      inherit config;
      cfg = config.services.minecraft;
      service =
        let
          unit = config.systemd.services.minecraft;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };

      paper =
        let
          config = evalConfig [
            ../images/games/minecraft
            versions."1.21.11-paper"
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          service =
            let
              unit = config.systemd.services.minecraft;
            in
            {
              inherit unit;
              config = unit.serviceConfig;
            };
          managed = {
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
            dropins = config.environment.etc."minecraft/managed-dropins".source;
          };
        };

      rcon =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft.rcon.enable = true;
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          managed.serverFiles = config.environment.etc."minecraft/managed-server-files".source;

          openFirewall =
            let
              config = evalConfig [
                ../images/games/minecraft
                defaultMinecraftModule
                {
                  services.minecraft.rcon = {
                    enable = true;
                    port = 25576;
                    openFirewall = true;
                  };
                }
              ];
            in
            {
              inherit config;
              cfg = config.services.minecraft;
            };
        };

      paperPlugins =
        let
          config = evalConfig [
            ../images/games/minecraft
            versions."26.1.2-paper"
            {
              services.minecraft.plugins = {
                pvpindex-factions = { };
                simple-voice-chat.port = 24455;
                terraformgenerator.worlds = [
                  "factions"
                  "factions_nether"
                  "factions_the_end"
                ];
                worldedit = { };
              };
              services.minecraft.properties.level-name = "factions";
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
        };

      nestedProperties =
        let
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft.properties = {
                query = {
                  port = 25565;
                };
                rcon = {
                  port = 25575;
                };
              };
            }
          ];
        in
        {
          inherit config;
          managed.serverFiles = config.environment.etc."minecraft/managed-server-files".source;
        };

      access =
        let
          json = pkgs.formats.json { };
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft = {
                whitelist.enable = true;
                players = {
                  Alice = {
                    uuid = "00000000-0000-0000-0000-000000000001";
                    whitelist = true;
                    operator = {
                      enable = true;
                      level = 3;
                      bypassesPlayerLimit = true;
                    };
                  };

                  Bob = {
                    uuid = "00000000-0000-0000-0000-000000000002";
                    whitelist = true;
                  };
                };
              };
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          fixtures = {
            whitelist = {
              current = json.generate "minecraft-whitelist-current.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000003";
                  name = "Manual";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000004";
                  name = "Removed";
                }
              ];

              previous = json.generate "minecraft-whitelist-previous.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000004";
                  name = "Removed";
                }
              ];
            };

            operators = {
              current = json.generate "minecraft-operators-current.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                  level = 1;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000005";
                  name = "ManualOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000006";
                  name = "RemovedOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
              ];

              previous = json.generate "minecraft-operators-previous.json" [
                {
                  uuid = "00000000-0000-0000-0000-000000000001";
                  name = "OldAlice";
                  level = 1;
                  bypassesPlayerLimit = false;
                }
                {
                  uuid = "00000000-0000-0000-0000-000000000006";
                  name = "RemovedOp";
                  level = 4;
                  bypassesPlayerLimit = false;
                }
              ];
            };
          };
          service =
            let
              unit = config.systemd.services.minecraft;
            in
            {
              inherit unit;
              config = unit.serviceConfig;
            };
          managed = {
            access = config.environment.etc."minecraft/managed-access".source;
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
          };
          syncManaged = ix.mkMinecraftSyncManaged {
            inherit pkgs;
            inherit (config.services.minecraft) dropinDir;
            dataDir = "/build/minecraft-access-data";
            managedRoot = "/build/minecraft-managed-root";
            plugmanReloadEnabled = false;
            rconEnabled = false;
            ignoredPlugins = [ ];
            rconPort = config.services.minecraft.rcon.port;
            rconPasswordFile = "/build/minecraft-access-data/.ix-rcon-password";
            rconBroadcastToOps = false;
          };
        };

      nbt =
        let
          tags = ix.minecraft.nbt;
          config = evalConfig [
            ../images/games/minecraft
            defaultMinecraftModule
            {
              services.minecraft = {
                serverFiles = {
                  "generated/example.snbt" = tags.compound {
                    DataVersion = tags.int 4325;
                    Enabled = tags.bool true;
                    Health = tags.short 20;
                    Angle = tags.float 0.5;
                    Precise = tags.double 12.25;
                    Flags = tags.byteArray [
                      1
                      0
                      (-1)
                    ];
                    Spawn = tags.compound {
                      Dimension = tags.string "minecraft:overworld";
                      Pos = tags.list [
                        (tags.double 1.5)
                        (tags.double 65.25)
                        (tags.double (-30.5))
                      ];
                    };
                  };

                  "generated/example.nbt" = tags.root "ix" (
                    tags.compound {
                      Name = tags.string "binary";
                      Values = tags.intArray [
                        1
                        2
                        3
                      ];
                    }
                  );

                  "generated/example.nbt.gz" = tags.compound {
                    Name = tags.string "compressed";
                  };
                };

                configFiles."generated/client.snbt" = tags.compound {
                  Side = tags.string "config";
                };
              };
            }
          ];
        in
        {
          inherit config;
          cfg = config.services.minecraft;
          managed = {
            config = config.environment.etc."minecraft/managed-config".source;
            serverFiles = config.environment.etc."minecraft/managed-server-files".source;
          };
        };
    };

  bedrock =
    let
      config = evalConfig [ ../images/games/minecraft-bedrock ];
    in
    {
      inherit config;
      cfg = config.services.minecraft-bedrock;
      service =
        let
          unit = config.systemd.services.minecraft-bedrock;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };
    };

  remoteDesktop =
    let
      config = evalConfig [ ../images/desktop/remote-desktop ];
    in
    {
      inherit config;
      cfg = config.services.remote-desktop;
      service =
        let
          unit = config.systemd.services.remote-desktop;
        in
        {
          inherit unit;
          config = unit.serviceConfig;
        };
    };

  kernelDev =
    let
      config = evalConfig [ ../images/dev/kernel-dev ];
    in
    {
      inherit config;
      git.clone = {
        service = config.systemd.services.git-clone;
        timer = config.systemd.timers.git-clone;
      };
    };

  pythonAppClosureProbe = ix.writePythonApplication pkgs {
    name = "python-app-closure-probe";
    src = pkgs.writeText "python-app-closure-probe.py" ''
      print("python app source is in the runtime closure")
    '';
    check = false;
  };

  cargoUnitFixture = fs.toSource {
    root = ./fixtures/cargo-unit-hello;
    fileset = fs.unions [
      ./fixtures/cargo-unit-hello/Cargo.lock
      ./fixtures/cargo-unit-hello/Cargo.toml
      ./fixtures/cargo-unit-hello/src
    ];
  };

  cargoUnitWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    cargoArgs = [
      "--bin"
      "cargo-unit-hello"
    ];
  };

  cargoUnitHello = cargoUnitWorkspace.binaries.cargo-unit-hello;

  cargoUnitPolicyDisabledWorkspace = ix.cargoUnit.buildWorkspace {
    src = cargoUnitFixture;
    cargoArgs = [
      "--bin"
      "cargo-unit-hello"
    ];
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
    };
  };

  bunSiteFixture = fs.toSource {
    root = ./fixtures/bun-site;
    fileset = fs.unions [
      ./fixtures/bun-site/bin
      ./fixtures/bun-site/bun.lock
      ./fixtures/bun-site/package.json
    ];
  };

  bunSite = ix.buildBunSite pkgs {
    pname = "bun-site-fixture";
    version = "0.1.0";
    src = bunSiteFixture;
  };

  bunLockPackage = builtins.head bunSite.bunNodeModules.bunCache.lock.packages;

  fleet = ix.mkFleet {
    deployment.region = "hil-1";
    secrets.sessionKey.generate = true;

    nodes = {
      db = {
        services.ix-postgresql.enable = true;
      };

      web = {
        tags = [ "public" ];
        deployment = {
          destination = "fleet-web:latest";
          ipv4 = true;
        };
        modules = [
          (
            { nodes, ... }:
            {
              services.remote-desktop.enable = true;
              environment.etc."db-host".text = nodes.db.config.networking.hostName;
            }
          )
        ];
      };

      worker = {
        replicas = 2;
        dependsOn = [ "db" ];
        modules = [
          {
            services.remote-desktop.enable = true;
          }
        ];
      };
    };
  };

  fleetPlan = fleet.planValue.nodes;

  extendedAttributes =
    let
      config = evalConfig [
        {
          ix.extendedAttributes."/build/ix-xattr-test" = {
            create = true;
            attributes = {
              "user.ix.kind" = "test.path";
              "user.ix.owner" = "ix";
            };
          };
        }
      ];
    in
    {
      inherit config;
      activationScript = config.system.activationScripts.ix-extended-attributes.text;
    };

  # --- Per-image assertion groups -------------------------------------------

  groups = {
    extended-attributes = [
      {
        assertion = builtins.hasAttr "/build/ix-xattr-test" extendedAttributes.config.ix.extendedAttributes;
        message = "generic ix.extendedAttributes should expose absolute runtime paths";
      }
      {
        assertion = builtins.elem pkgs.attr extendedAttributes.config.environment.systemPackages;
        message = "generic ix.extendedAttributes should add attr tools for runtime inspection";
      }
      {
        assertion =
          lib.hasInfix "/bin/setfattr" extendedAttributes.activationScript
          && lib.hasInfix "user.ix.kind" extendedAttributes.activationScript;
        message = "generic ix.extendedAttributes should render setfattr activation commands";
      }
      {
        assertion = lib.hasInfix "refusing to set extended attributes on symlink" extendedAttributes.activationScript;
        message = "generic ix.extendedAttributes should avoid following symlinks";
      }
    ];

    kernel-dev = [
      {
        assertion = kernelDev.config.ix.image.name == "linux-kernel-dev";
        message = "kernel-dev image should set the expected OCI image name";
      }
      {
        assertion = kernelDev.config.services.git-clone.enable;
        message = "kernel-dev image should enable first-boot git cloning";
      }
      {
        assertion = kernelDev.config.services.git-clone.url == "https://github.com/torvalds/linux.git";
        message = "kernel-dev image should clone the Linux repository";
      }
      {
        assertion = kernelDev.config.services.git-clone.dest == "/src/linux";
        message = "kernel-dev image should clone Linux into /src/linux";
      }
      {
        assertion = kernelDev.config.services.git-clone.activation == "timer";
        message = "kernel-dev image should clone Linux after boot readiness";
      }
      {
        assertion = kernelDev.git.clone.service.wantedBy == [ ];
        message = "timer-activated git clone should not be wanted by multi-user.target";
      }
      {
        assertion = kernelDev.git.clone.timer.wantedBy == [ "timers.target" ];
        message = "timer-activated git clone should be started by timers.target";
      }
    ];

    minecraft = [
      {
        assertion = minecraft.config.ix.image.tag == defaultMinecraftVersion;
        message = "default minecraft image tag should follow versions.nix default";
      }
      {
        assertion = defaultMinecraftVersion == "26.1.2-fabric";
        message = "default minecraft image should use the stable 26.1.2 Fabric variant";
      }
      {
        assertion = minecraft.cfg.properties."max-players" == 100000;
        message = "default minecraft image should allow the large ix player ceiling";
      }
      {
        assertion =
          minecraft.cfg.properties."online-mode" && minecraft.cfg.properties."enforce-secure-profile";
        message = "default minecraft image should keep account authentication and secure profiles explicit";
      }
      {
        assertion =
          minecraft.cfg.properties.gamemode == "survival"
          && !minecraft.cfg.properties."force-gamemode"
          && minecraft.cfg.properties.pvp
          && !minecraft.cfg.properties.hardcore
          && minecraft.cfg.properties."spawn-protection" == 16
          && !minecraft.cfg.properties."allow-flight"
          && !minecraft.cfg.properties."enable-command-block";
        message = "default minecraft image should keep conservative gameplay and command defaults";
      }
      {
        assertion =
          minecraft.cfg.properties."view-distance" == 32
          && minecraft.cfg.properties."simulation-distance" == 32;
        message = "default minecraft image should use the high-distance template defaults";
      }
      {
        assertion = lib.all (slug: builtins.hasAttr slug minecraft.config.services.minecraft.mods) [
          "fabric-api"
          "lithium"
          "c2me-fabric"
          "spark"
          "grimac"
        ];
        message = "default minecraft image should include the 26.1.2 Fabric server mod set";
      }
      {
        assertion = minecraft.config.services.minecraft.javaPackage == pkgs.temurin-jre-bin-25;
        message = "default Fabric minecraft should use Temurin";
      }
      {
        assertion = lib.hasInfix "/bin/java" minecraft.service.config.ExecStart;
        message = "minecraft ExecStart should launch Java";
      }
      {
        assertion = lib.hasInfix "-XX:MaxRAMPercentage=85" minecraft.service.config.ExecStart;
        message = "minecraft should use MaxRAMPercentage for auto-scaling heap";
      }
      {
        assertion = lib.hasInfix "-XX:+UseG1GC" minecraft.service.config.ExecStart;
        message = "minecraft should include the default modern server GC flags";
      }
      {
        assertion =
          lib.hasInfix "-jar" minecraft.service.config.ExecStart
          && lib.hasInfix "nogui" minecraft.service.config.ExecStart;
        message = "minecraft ExecStart should launch the configured server jar in nogui mode";
      }
      {
        assertion = lib.hasInfix "minecraft-hot-reload-agent.jar=socket=/run/minecraft-hot-reload/socket" minecraft.service.config.ExecStart;
        message = "Fabric minecraft should start the hot reload Java agent";
      }
      {
        assertion = minecraft.service.config.RuntimeDirectory == "minecraft-hot-reload";
        message = "Fabric minecraft should create a runtime directory for the hot reload socket";
      }
      {
        assertion = builtins.length minecraft.service.unit.reloadTriggers == 3;
        message = "minecraft managed files should trigger systemd reloads rather than unit restarts";
      }
      {
        assertion = lib.hasInfix "minecraft-sync-managed" minecraft.service.unit.preStart;
        message = "minecraft preStart should sync managed files from /etc";
      }
      {
        assertion = !(lib.hasInfix "fabric-api" minecraft.service.unit.preStart);
        message = "minecraft preStart should not embed managed mod store paths in the unit";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft".attributes."user.ix.kind"
          == "minecraft.server-root";
        message = "minecraft should label its runtime data directory through the generic xattr module";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/region".attributes."user.ix.kind"
          == "minecraft.region-directory"
          &&
            minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/region".attributes."user.ix.minecraft.dimension"
            == "overworld";
        message = "minecraft should label overworld region directories through the generic xattr module";
      }
      {
        assertion =
          minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/DIM-1/region".attributes."user.ix.minecraft.dimension"
          == "nether"
          &&
            minecraft.config.ix.extendedAttributes."/var/lib/minecraft/world/DIM1/region".attributes."user.ix.minecraft.dimension"
            == "end";
        message = "minecraft should label Nether and End region directories through the generic xattr module";
      }
      # rcon coverage stays on the minecraft default image because the option
      # surface lives in `services.minecraft`, not in a paper-specific module.
      {
        assertion = minecraft.rcon.cfg.rcon.enable;
        message = "minecraft RCON should be enabled through a typed option";
      }
      {
        assertion = minecraft.rcon.cfg.rcon.passwordFile == "/var/lib/minecraft/.ix-rcon-password";
        message = "minecraft RCON should default to a state-local password file";
      }
      {
        assertion = !(minecraft.rcon.cfg.properties ? "rcon.password");
        message = "typed minecraft RCON should not put the password in Nix-managed server.properties";
      }
      {
        assertion =
          minecraft.rcon.config.networking.firewall.allowedTCPPorts == [ minecraft.rcon.cfg.port ];
        message = "typed minecraft RCON should keep the RCON port private by default";
      }
      {
        assertion =
          minecraft.rcon.openFirewall.config.networking.firewall.allowedTCPPorts == [
            minecraft.rcon.openFirewall.cfg.port
            minecraft.rcon.openFirewall.cfg.rcon.port
          ];
        message = "typed minecraft RCON should open the firewall only when requested";
      }
      {
        assertion = minecraft.access.cfg.properties.white-list;
        message = "typed minecraft whitelist should enable server.properties white-list";
      }
      {
        assertion = minecraft.access.cfg.properties.enforce-whitelist;
        message = "typed minecraft whitelist should enable enforce-whitelist by default";
      }
      {
        assertion = !(minecraft.access.cfg.serverFiles ? "whitelist.json");
        message = "typed minecraft whitelist should not symlink the mutable whitelist file through serverFiles";
      }
      {
        assertion = !(minecraft.access.cfg.serverFiles ? "ops.json");
        message = "typed minecraft operators should not symlink the mutable ops file through serverFiles";
      }
      {
        assertion = builtins.elem minecraft.access.managed.access minecraft.access.service.unit.restartTriggers;
        message = "typed minecraft access changes should restart the server so Minecraft rereads mutable access files";
      }
      {
        assertion = builtins.hasAttr "generated/example.snbt" minecraft.nbt.cfg.serverFiles;
        message = "minecraft serverFiles should accept readable SNBT files";
      }
      {
        assertion = builtins.hasAttr "generated/example.nbt" minecraft.nbt.cfg.serverFiles;
        message = "minecraft serverFiles should accept binary NBT files";
      }
      {
        assertion = builtins.hasAttr "generated/client.snbt" minecraft.nbt.cfg.configFiles;
        message = "minecraft configFiles should accept readable SNBT files";
      }
    ];

    "minecraft_1.21.11-paper" = [
      {
        assertion = minecraft.paper.cfg.dropinDir == "plugins";
        message = "Paper minecraft should use the plugins drop directory";
      }
      {
        assertion = builtins.length minecraft.paper.service.unit.reloadTriggers == 3;
        message = "Paper minecraft managed plugins should trigger systemd reloads";
      }
      {
        assertion = !(minecraft.paper.service.config ? RuntimeDirectory);
        message = "Paper minecraft should not start the JVM hot reload socket";
      }
      {
        assertion =
          minecraft.paper.cfg.autoReload.rconPasswordFile == "/var/lib/minecraft/.ix-rcon-password";
        message = "Paper minecraft should use a state-local RCON password file";
      }
      {
        assertion = !(minecraft.paper.cfg.properties ? "rcon.password");
        message = "Paper minecraft should not put the RCON password in Nix-managed server.properties";
      }
      {
        assertion =
          minecraft.paper.config.networking.firewall.allowedTCPPorts == [ minecraft.paper.cfg.port ];
        message = "Paper minecraft should not expose the local RCON reload port through the firewall";
      }
    ];

    "minecraft_26.1.2-paper" = [
      {
        assertion = builtins.hasAttr "pvpindex-factions" minecraft.paperPlugins.cfg.pluginCatalog;
        message = "Paper minecraft should seed pluginCatalog from the generated 26.1.2 Paper catalog";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.pluginCatalog.pvpindex-factions.pluginName == "PvPIndexFactions";
        message = "Generated Paper plugin catalog should preserve Bukkit plugin names";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.worldedit.pluginName == "WorldEdit";
        message = "Paper minecraft should expose the generated WorldEdit plugin entry";
      }
      {
        assertion = minecraft.paperPlugins.cfg.pluginCatalog.simple-voice-chat.pluginName == "voicechat";
        message = "Generated Simple Voice Chat plugin entry should use its Bukkit runtime name";
      }
      {
        assertion = builtins.elem 24455 minecraft.paperPlugins.config.networking.firewall.allowedUDPPorts;
        message = "Simple Voice Chat should open its UDP port when installed as a Paper plugin";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.serverFiles."plugins/voicechat/voicechat-server.properties".port
          == 24455;
        message = "Simple Voice Chat should render Paper plugin config under plugins/voicechat";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.worlds.factions.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.worlds.factions_nether.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.worlds.factions_the_end.generator == "TerraformGenerator";
        message = "TerraformGenerator should bind every configured world to its generator";
      }
      {
        assertion =
          minecraft.paperPlugins.cfg.bukkit.worlds.factions.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.bukkit.worlds.factions_nether.generator == "TerraformGenerator"
          && minecraft.paperPlugins.cfg.bukkit.worlds.factions_the_end.generator == "TerraformGenerator";
        message = "Minecraft worlds should render to bukkit.yml world generator entries";
      }
    ];

    minecraft-bedrock = [
      {
        assertion = bedrock.config.ix.image.name == "minecraft-bedrock";
        message = "minecraft-bedrock image should set the expected OCI image name";
      }
      {
        assertion = bedrock.config.ix.image.tag == "1.26.14.1";
        message = "minecraft-bedrock image tag should follow the pinned Bedrock server version";
      }
      {
        assertion = bedrock.cfg.enable;
        message = "minecraft-bedrock image should enable services.minecraft-bedrock";
      }
      {
        assertion = bedrock.cfg.settings."server-name" == "ix-powered Bedrock";
        message = "minecraft-bedrock should set the expected default server name";
      }
      {
        assertion =
          bedrock.cfg.settings."server-port" == bedrock.cfg.port
          && bedrock.cfg.settings."server-portv6" == bedrock.cfg.portv6;
        message = "minecraft-bedrock server.properties should follow the configured UDP ports";
      }
      {
        assertion =
          bedrock.config.networking.firewall.allowedUDPPorts == [
            bedrock.cfg.port
            bedrock.cfg.portv6
          ];
        message = "minecraft-bedrock firewall should open only the configured UDP ports";
      }
      {
        assertion = bedrock.service.unit.description == "Minecraft Bedrock server";
        message = "minecraft-bedrock should run a dedicated systemd service";
      }
      {
        assertion = lib.hasInfix "/bin/bedrock_server" bedrock.service.config.ExecStart;
        message = "minecraft-bedrock ExecStart should launch bedrock_server";
      }
      {
        assertion = bedrock.service.config.StateDirectory == "minecraft-bedrock";
        message = "minecraft-bedrock service should get a managed state directory";
      }
    ];

    remote-desktop = [
      {
        assertion = remoteDesktop.config.ix.image.name == "ix-remote-desktop";
        message = "remote-desktop image should set the expected OCI image name";
      }
      {
        assertion = remoteDesktop.cfg.enable;
        message = "remote-desktop image should enable services.remote-desktop";
      }
      {
        assertion = remoteDesktop.cfg.package == pkgs.xpra;
        message = "remote-desktop should default to the nixpkgs Xpra package";
      }
      {
        assertion = remoteDesktop.cfg.port == 6080;
        message = "remote-desktop should expose the Xpra HTML5 client on port 6080";
      }
      {
        assertion = remoteDesktop.cfg.bindAddress == "0.0.0.0";
        message = "remote-desktop should bind browser clients on all interfaces by default";
      }
      {
        assertion = remoteDesktop.cfg.display == ":100";
        message = "remote-desktop should let Xpra own a deterministic virtual display";
      }
      {
        assertion = remoteDesktop.cfg.resolution == "1920x1080";
        message = "remote-desktop should default to a browser-friendly 1080p display";
      }
      {
        assertion = remoteDesktop.cfg.auth == "none";
        message = "remote-desktop should keep the current unauthenticated ix image contract explicit";
      }
      {
        assertion = remoteDesktop.service.unit.description == "Xpra remote desktop";
        message = "remote-desktop should run a single Xpra service";
      }
      {
        assertion = remoteDesktop.service.config.User == "remote-desktop";
        message = "remote-desktop service should run as its dedicated system user";
      }
      {
        assertion = remoteDesktop.service.config.StateDirectory == "remote-desktop";
        message = "remote-desktop service should get a managed state directory";
      }
      {
        assertion = remoteDesktop.service.config.RuntimeDirectory == "remote-desktop";
        message = "remote-desktop service should get a managed runtime directory";
      }
      {
        assertion = remoteDesktop.config.users.users.remote-desktop.isSystemUser;
        message = "remote-desktop user should be a system user";
      }
      {
        assertion = remoteDesktop.config.networking.firewall.allowedTCPPorts == [ remoteDesktop.cfg.port ];
        message = "remote-desktop firewall should open only the configured browser port";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? xvfb);
        message = "remote-desktop should not use a standalone Xvfb service";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? x11vnc);
        message = "remote-desktop should not use x11vnc";
      }
      {
        assertion = !(remoteDesktop.config.systemd.services ? novnc);
        message = "remote-desktop should not use a separate noVNC websockify service";
      }
    ];

    helpers = [
      {
        assertion = cargoUnitWorkspace.policyChecks ? unusedCrateDependencies;
        message = "cargo-unit workspaces should expose an unused dependency policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.policyChecks ? cargoAudit;
        message = "cargo-unit workspaces should expose a cargo-audit policy check by default";
      }
      {
        assertion = cargoUnitWorkspace.binaries.cargo-unit-hello ? unchecked;
        message = "cargo-unit package outputs should be wrapped by policy checks by default";
      }
      {
        assertion = cargoUnitPolicyDisabledWorkspace.policyChecks == { };
        message = "cargo-unit policy checks should be disableable for generated workspaces";
      }
      {
        assertion =
          bunLockPackage.name == "clsx"
          && bunLockPackage.version == "2.1.1"
          && lib.hasPrefix "sha512-" bunLockPackage.integrity;
        message = "bun lock helper should derive registry fetch metadata from bun.lock";
      }
    ];

    fleet = [
      {
        assertion = fleet.nodes.db.networking.hostName == "db";
        message = "fleet nodes should default hostName to the node name";
      }
      {
        assertion = fleet.nodes.db.ix.networking.eastWest.hostName == "db";
        message = "fleet nodes should expose their east-west host name through ix.networking";
      }
      {
        assertion = fleet.nodes.web.environment.etc."db-host".text == "db";
        message = "fleet node modules should be able to reference nodes.<name>.config";
      }
      {
        assertion = fleet.nodes.db.services.ix-postgresql.enable;
        message = "fleet plain attrset nodes should be treated as modules";
      }
      {
        assertion =
          fleetPlan.web.bootstrapImage == "registry.ix.dev/ix/test-cluster-bootstrap:zstd-tools-2026-05-12";
        message = "fleet switches should create missing nodes from the shared NixOS bootstrap image";
      }
      {
        assertion = fleetPlan.web.replacementImage.destination == "fleet-web:latest";
        message = "fleet wrapped-node deployment destination should flow into the replacement image plan";
      }
      {
        assertion = fleetPlan.web.system == "${fleet.nodes.web.system.build.toplevel}";
        message = "fleet plans should expose the NixOS system closure for switch";
      }
      {
        assertion = fleet.systemPackages.web-system == fleet.nodes.web.system.build.toplevel;
        message = "fleet system package outputs should match default source switch installables";
      }
      {
        assertion = fleet.packages.web == fleet.nodes.web.ix.build.ociImage;
        message = "fleet replacement package outputs should keep node names";
      }
      {
        assertion =
          fleetPlan.web.switch == {
            target = builtins.unsafeDiscardStringContext fleet.nodes.web.system.build.toplevel.drvPath;
            buildOn = "remote";
            buildVm = null;
            sourceInstallable = ".#web-system";
            overrideInputs = { };
          };
        message = "fleet plans should default to local eval and remote build switch metadata";
      }
      {
        assertion =
          fleetPlan.web.replacementImage.sourceDrv
          == builtins.unsafeDiscardStringContext fleet.nodes.web.ix.build.ociImage.drvPath;
        message = "fleet plans should expose replacement image derivations without forcing local image builds";
      }
      {
        assertion = fleetPlan.web.region == "hil-1";
        message = "fleet nodes should inherit the top-level deployment region";
      }
      {
        assertion = fleetPlan.web.tags == [ "public" ];
        message = "fleet wrapped-node tags should flow into the generated plan";
      }
      {
        assertion = fleetPlan.web.ipv4;
        message = "fleet wrapped-node deployment overrides should flow into the generated plan";
      }
      {
        assertion = fleet.planValue.secrets.sessionKey.generate;
        message = "fleet plans should carry declarative secret specs";
      }
      {
        assertion = fleetPlan."worker-0".baseName == "worker" && fleetPlan."worker-1".replicaIndex == 1;
        message = "fleet replicas should expand into stable node identities";
      }
      {
        assertion = fleetPlan."worker-0".dependsOn == [ "db" ];
        message = "fleet replica dependencies should point at expanded node identities";
      }
    ];
  };

  # --- Per-image build-time checks ------------------------------------------

  buildScripts = {
    extended-attributes = ''
      rm -rf /build/ix-xattr-test
      mkdir -p /build/ix-xattr-probe
      if ${pkgs.attr}/bin/setfattr --name user.ix.probe --value yes -- /build/ix-xattr-probe; then
        ${extendedAttributes.activationScript}
        test -d /build/ix-xattr-test
        test "$(${pkgs.attr}/bin/getfattr --absolute-names --only-values -n user.ix.kind /build/ix-xattr-test)" = "test.path"
        test "$(${pkgs.attr}/bin/getfattr --absolute-names --only-values -n user.ix.owner /build/ix-xattr-test)" = "ix"
      else
        echo "xattrs are not supported by the Nix build sandbox filesystem; checked activation rendering by eval"
      fi
    '';

    minecraft = ''
      ! grep -R 'rcon.password' ${minecraft.rcon.managed.serverFiles}
      grep -q '^query.port=25565$' ${minecraft.nestedProperties.managed.serverFiles}/server.properties
      grep -q '^rcon.port=25575$' ${minecraft.nestedProperties.managed.serverFiles}/server.properties
      grep -q '^white-list=true$' ${minecraft.access.managed.serverFiles}/server.properties
      grep -q '^enforce-whitelist=true$' ${minecraft.access.managed.serverFiles}/server.properties
      grep -q 'factions_nether:' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q 'factions_the_end:' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q 'generator: TerraformGenerator' ${
        minecraft.paperPlugins.config.environment.etc."minecraft/managed-server-files".source
      }/bukkit.yml
      grep -q '"name": "Alice"' ${minecraft.access.managed.access}/whitelist.json
      grep -q '"name": "Bob"' ${minecraft.access.managed.access}/whitelist.json
      grep -q '"level": 3' ${minecraft.access.managed.access}/ops.json
      grep -q '"bypassesPlayerLimit": true' ${minecraft.access.managed.access}/ops.json

      rm -rf /build/minecraft-access-data /build/minecraft-managed-root
      mkdir -p /build/minecraft-access-data/.ix-managed-access /build/minecraft-managed-root
      ln -s ${minecraft.access.managed.access} /build/minecraft-managed-root/managed-access
      ln -s ${minecraft.access.managed.serverFiles} /build/minecraft-managed-root/managed-server-files
      cp ${minecraft.access.fixtures.whitelist.current} /build/minecraft-access-data/whitelist.json
      cp ${minecraft.access.fixtures.whitelist.previous} /build/minecraft-access-data/.ix-managed-access/whitelist.json
      cp ${minecraft.access.fixtures.operators.current} /build/minecraft-access-data/ops.json
      cp ${minecraft.access.fixtures.operators.previous} /build/minecraft-access-data/.ix-managed-access/ops.json

      ${lib.getExe minecraft.access.syncManaged}
      test ! -L /build/minecraft-access-data/whitelist.json
      test ! -L /build/minecraft-access-data/ops.json
      grep -q '"name": "Alice"' /build/minecraft-access-data/whitelist.json
      grep -q '"name": "Bob"' /build/minecraft-access-data/whitelist.json
      grep -q '"name": "Manual"' /build/minecraft-access-data/whitelist.json
      ! grep -q '"name": "Removed"' /build/minecraft-access-data/whitelist.json
      grep -q '"level": 3' /build/minecraft-access-data/ops.json
      grep -q '"bypassesPlayerLimit": true' /build/minecraft-access-data/ops.json
      grep -q '"name": "ManualOp"' /build/minecraft-access-data/ops.json
      ! grep -q '"name": "RemovedOp"' /build/minecraft-access-data/ops.json

      grep -q 'DataVersion: 4325' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Enabled: 1B' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Health: 20S' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Angle: 0.5F' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Precise: 12.25' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'B;' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Dimension: "minecraft:overworld"' ${minecraft.nbt.managed.serverFiles}/generated/example.snbt
      grep -q 'Side: config' ${minecraft.nbt.managed.config}/generated/client.snbt
      test "$(od -An -tx1 -N5 ${minecraft.nbt.managed.serverFiles}/generated/example.nbt | tr -d ' \n')" = "0a00026978"
      test "$(od -An -tx1 -N2 ${minecraft.nbt.managed.serverFiles}/generated/example.nbt.gz | tr -d ' \n')" = "1f8b"
    '';

    "minecraft_1.21.11-paper" = ''
      grep -q 'ignored-plugins' ${minecraft.paper.managed.serverFiles}/plugins/PlugManX/config.yml
      grep -q 'PlugManX' ${minecraft.paper.managed.serverFiles}/plugins/PlugManX/config.yml
      ! grep -R 'rcon.password' ${minecraft.paper.managed.serverFiles}
      grep -q '^almanac$' ${minecraft.paper.managed.dropins}/almanac.jar.plugin-name
      grep -q '^PlugManX$' ${minecraft.paper.managed.dropins}/PlugManX.jar.plugin-name
      grep -q -- '--password-file "/var/lib/minecraft/.ix-rcon-password"' ${minecraft.paper.service.config.ExecReload}
      grep -q 'plugman $row.action $row.plugin' ${minecraft.paper.service.config.ExecReload}
      ! grep -q 'reload all' ${minecraft.paper.service.config.ExecReload}
    '';
  };

  helperScript = ''
    ${lib.getExe pythonAppClosureProbe} > python-app-closure-probe.out
    grep -q 'python app source is in the runtime closure' python-app-closure-probe.out

    ${cargoUnitHello}/bin/cargo-unit-hello > cargo-unit-hello.out
    grep -q 'hello from cargo-unit' cargo-unit-hello.out

    grep -q 'class="ix bun"' ${bunSite}/share/bun-site-fixture/index.html
    test -d ${bunSite.bunNodeModules}/node_modules/clsx
    test -x ${bunSite.bunNodeModules.nodeCompat}/bin/node
  '';

  # --- Test derivation builder ----------------------------------------------

  mkTest =
    name: assertions: extraScript:
    let
      failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
    in
    assert lib.assertMsg (failures == [ ]) (
      "ix-test-${name}:\n  " + lib.concatStringsSep "\n  " failures
    );
    pkgs.runCommand "ix-test-${name}" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''
      ${extraScript}
      mkdir -p "$out"
    '';

  imageTests = lib.mapAttrs (name: assertions: mkTest name assertions (buildScripts.${name} or "")) (
    builtins.removeAttrs groups [ "fleet" ]
  );

  fleetTest = mkTest "fleet" groups.fleet "";

  helperTest = pkgs.runCommand "ix-test-helpers" { nativeBuildInputs = [ pkgs.gnugrep ]; } ''
    ${helperScript}
    mkdir -p "$out"
  '';
in
{
  inherit imageTests;

  # Aggregate. Pulls every per-image test into one derivation so
  # `nix flake check` covers the whole suite.
  eval = pkgs.linkFarmFromDrvs "ix-images-eval-tests" (
    lib.attrValues imageTests
    ++ [
      fleetTest
      helperTest
    ]
  );
}
